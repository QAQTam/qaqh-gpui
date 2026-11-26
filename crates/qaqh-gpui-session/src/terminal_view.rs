//! 原生 GPUI 虚拟终端视图与画布元素(clean-room 自研,对标 Zed `TerminalElement`)。
//!
//! 架构分层:
//! - [`TerminalView`]:实体状态层,持有 [`TerminalEmulator`]、键盘/粘贴路由、`EntityInputHandler`
//!   (Windows 中文 IME 组合输入与选区锚点)、鼠标拖拽选区与滚轮回滚。
//! - `TerminalCanvasElement`:GPUI [`Element`] 绘制层,在 `prepaint` 测量等宽字形栅格、
//!   同步动态行列尺寸、按样式合批绘制背景/选区 Quad 与列锚定 `ShapedLine`,并叠加光标、
//!   IME preedit 候选浮条与回滚滚动条指示器。

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use alacritty_terminal::selection::SelectionType;
use alacritty_terminal::term::TermMode;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::index::Side;
use alacritty_terminal::vte::ansi::CursorShape;
use gpui_kit::prelude::*;
use gpui_kit::{
    App, BorderStyle, Bounds, ClipboardItem, ContentMask, Context, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Font, FontStyle, FontWeight,
    GlobalElementId, Hsla, InspectorElementId, KeyDownEvent, LayoutId, MouseButton,
    MouseDownEvent, MouseMoveEvent, PaintQuad, Pixels, Point, Render, ScrollDelta, ScrollEvent,
    ShapedLine, SharedString, StrikethroughStyle, Style as LayoutStyle, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, div, fill, outline, point, px, relative, size,
};
use qaqh_gpui_kit::kit::soft;
use qaqh_gpui_kit::settings;
use qaqh_gpui_kit::theme::{ActiveTheme, terminal_palette};

use crate::terminal::{
    DEFAULT_COLS, DEFAULT_ROWS, TerminalEmulator, encode_keystroke, encode_paste,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColsMode {
    AutoFit,
    FixedCols(usize),
}

fn mono_font(weight: FontWeight, style: FontStyle) -> Font {
    Font {
        family: qaqh_gpui_kit::fonts::mono_font_family(),
        features: Default::default(),
        fallbacks: qaqh_gpui_kit::fonts::mono_font_fallbacks(),
        weight,
        style,
    }
}

/// 测量等宽字体在指定字号下的单格宽度(10 字符均摊,避免 10px 小字号下钳制 6px 导致列漂移)。
pub fn measure_mono_cell_width(font_size_px: f32, window: &mut Window) -> f32 {
    let font_size = px(font_size_px.clamp(8.5, 24.0));
    let probe = window.text_system().shape_line(
        SharedString::from("MMMMMMMMMM"),
        font_size,
        &[TextRun {
            len: 10,
            font: mono_font(FontWeight::NORMAL, FontStyle::Normal),
            color: gpui_kit::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    (f32::from(probe.width()) / 10.0).max(4.0)
}

type InputCallback = Arc<dyn Fn(Vec<u8>, &mut App) + Send + Sync>;
type ResizeCallback = Arc<dyn Fn(u16, u16, &mut App) + Send + Sync>;

#[derive(Clone, Copy)]
struct TerminalMetrics {
    origin_x: f32,
    origin_y: f32,
    viewport_width: f32,
    scroll_x_px: f32,
    cell_width: f32,
    line_height: f32,
    cols: usize,
    rows: usize,
    cursor_col: usize,
    cursor_row: usize,
}

impl Default for TerminalMetrics {
    fn default() -> Self {
        Self {
            origin_x: 0.0,
            origin_y: 0.0,
            viewport_width: 640.0,
            scroll_x_px: 0.0,
            cell_width: 8.0,
            line_height: 18.0,
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            cursor_col: 0,
            cursor_row: 0,
        }
    }
}

/// 可复用终端视口实体:交互 shell 传输入/尺寸回调;只读 exec 面板传 `new_readonly`。
pub struct TerminalView {
    id: SharedString,
    emulator: TerminalEmulator,
    interactive: bool,
    cols_mode: ColsMode,
    font_size_override: Option<f32>,
    focus_handle: FocusHandle,
    metrics: Rc<Cell<TerminalMetrics>>,
    marked_text: Option<String>,
    selecting: bool,
    scroll_remainder_px: f32,
    scroll_x_px: f32,
    on_input: Option<InputCallback>,
    on_resize: Option<ResizeCallback>,
}

impl TerminalView {
    pub fn new_interactive(
        id: impl Into<SharedString>,
        focus_handle: FocusHandle,
        on_input: impl Fn(Vec<u8>, &mut App) + Send + Sync + 'static,
        on_resize: impl Fn(u16, u16, &mut App) + Send + Sync + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            emulator: TerminalEmulator::new(DEFAULT_COLS, DEFAULT_ROWS),
            interactive: true,
            cols_mode: ColsMode::AutoFit,
            font_size_override: None,
            focus_handle,
            metrics: Rc::new(Cell::new(TerminalMetrics::default())),
            marked_text: None,
            selecting: false,
            scroll_remainder_px: 0.0,
            scroll_x_px: 0.0,
            on_input: Some(Arc::new(on_input)),
            on_resize: Some(Arc::new(on_resize)),
        }
    }

    pub fn new_readonly(id: impl Into<SharedString>, focus_handle: FocusHandle) -> Self {
        Self {
            id: id.into(),
            emulator: TerminalEmulator::new(DEFAULT_COLS, DEFAULT_ROWS),
            interactive: false,
            cols_mode: ColsMode::AutoFit,
            font_size_override: None,
            focus_handle,
            metrics: Rc::new(Cell::new(TerminalMetrics::default())),
            marked_text: None,
            selecting: false,
            scroll_remainder_px: 0.0,
            scroll_x_px: 0.0,
            on_input: None,
            on_resize: None,
        }
    }

    pub fn set_cols_mode(&mut self, mode: ColsMode) {
        if self.cols_mode == mode {
            return;
        }
        self.cols_mode = mode;
        if let ColsMode::FixedCols(cols) = mode {
            let rows = self.emulator.size().1 as usize;
            self.emulator.resize(cols, rows);
        }
    }

    pub fn set_font_size(&mut self, size: Option<f32>) {
        self.font_size_override = size.map(|v| v.clamp(8.5, 24.0));
    }

    pub fn scroll_to_bottom(&mut self, cx: &mut Context<Self>) {
        self.emulator.scroll_to_bottom();
        cx.notify();
    }

    /// 喂入终端字节流,并将终端生成的应答序列(`CSI 6 n` 等)立刻回写远端 PTY。
    pub fn feed(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        if bytes.is_empty() {
            return;
        }
        let responses = self.emulator.feed(bytes);
        if self.interactive
            && let Some(on_input) = &self.on_input
        {
            for response in responses {
                if !response.is_empty() {
                    on_input(response, cx);
                }
            }
        }
        cx.notify();
    }

    /// 供父视图 `render` 内同步喂数(如只读 exec 面板),避免在绘制期多余触发 `notify`。
    pub fn feed_quiet(&mut self, bytes: &[u8]) {
        if !bytes.is_empty() {
            let _ = self.emulator.feed(bytes);
        }
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.reset_quiet();
        cx.notify();
    }

    pub fn reset_quiet(&mut self) {
        self.emulator.reset();
        self.marked_text = None;
        self.selecting = false;
        self.scroll_remainder_px = 0.0;
        self.scroll_x_px = 0.0;
    }

    pub fn size(&self) -> (u16, u16) {
        self.emulator.size()
    }

    pub fn has_output(&self) -> bool {
        self.emulator.has_output()
    }

    pub fn display_offset(&self) -> usize {
        self.emulator.display_offset()
    }

    pub fn title(&self) -> Option<String> {
        self.emulator.title()
    }

    pub fn selection_text(&self) -> Option<String> {
        self.emulator.selection_text()
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    fn send_bytes(&mut self, bytes: Vec<u8>, cx: &mut Context<Self>) {
        if !self.interactive || bytes.is_empty() {
            return;
        }
        self.emulator.clear_selection();
        self.emulator.scroll_to_bottom();
        if let Some(on_input) = &self.on_input {
            on_input(bytes, cx);
        }
    }

    fn point_to_cell(&self, position: Point<Pixels>) -> (usize, usize, Side) {
        let m = self.metrics.get();
        let cw = m.cell_width.max(1.0);
        let lh = m.line_height.max(1.0);
        let rel_x = (f32::from(position.x) - m.origin_x + m.scroll_x_px).max(0.0);
        let rel_y = (f32::from(position.y) - m.origin_y).max(0.0);
        let col = ((rel_x / cw).floor() as usize).min(m.cols.saturating_sub(1));
        let row = ((rel_y / lh).floor() as usize).min(m.rows.saturating_sub(1));
        let in_cell_x = rel_x - col as f32 * cw;
        let side = if in_cell_x > cw * 0.5 {
            Side::Right
        } else {
            Side::Left
        };
        (col, row, side)
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self.emulator.selection_text() else {
            return false;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.interactive {
            return false;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return false;
        };
        let encoded = encode_paste(&text, self.emulator.mode());
        self.send_bytes(encoded, cx);
        cx.notify();
        true
    }
}

impl EntityInputHandler for TerminalView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let marked = self.marked_text.as_deref().unwrap_or("");
        let utf16_len = marked.encode_utf16().count();
        let start = range_utf16.start.min(utf16_len);
        let end = range_utf16.end.min(utf16_len).max(start);
        *adjusted_range = Some(start..end);
        let slice: Vec<u16> = marked.encode_utf16().skip(start).take(end - start).collect();
        Some(String::from_utf16_lossy(&slice))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let len = self
            .marked_text
            .as_deref()
            .map_or(0, |text| text.encode_utf16().count());
        Some(UTF16Selection {
            range: len..len,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_text
            .as_deref()
            .map(|text| 0..text.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.marked_text.take().is_some() {
            cx.notify();
        }
    }

    fn paste(&mut self, item: ClipboardItem, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.interactive {
            return;
        }
        if let Some(text) = item.text() {
            let encoded = encode_paste(&text, self.emulator.mode());
            self.send_bytes(encoded, cx);
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = None;
        if self.interactive && !text.is_empty() {
            self.send_bytes(text.as_bytes().to_vec(), cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.marked_text = (!new_text.is_empty()).then(|| new_text.to_owned());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let m = self.metrics.get();
        Some(Bounds::new(
            point(
                px(m.origin_x - m.scroll_x_px + m.cursor_col as f32 * m.cell_width),
                px(m.origin_y + m.cursor_row as f32 * m.line_height),
            ),
            size(px(m.cell_width), px(m.line_height)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn accepts_text_input(&self, _window: &mut Window, _cx: &mut Context<Self>) -> bool {
        self.interactive
    }
}

impl Render for TerminalView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let interactive = self.interactive;
        let entity = cx.entity();
        div()
            .id(self.id.clone())
            .flex_1()
            .min_h_0()
            .w_full()
            .h_full()
            .overflow_hidden()
            .cursor_text()
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window, cx);
                    let (col, row, side) = this.point_to_cell(event.position);
                    let ty = match event.click_count {
                        2 => SelectionType::Semantic,
                        n if n >= 3 => SelectionType::Lines,
                        _ if event.modifiers.alt => SelectionType::Block,
                        _ => SelectionType::Simple,
                    };
                    this.emulator.start_selection(ty, col, row, side);
                    this.selecting = true;
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if !this.selecting || event.pressed_button != Some(MouseButton::Left) {
                    return;
                }
                let (col, row, side) = this.point_to_cell(event.position);
                this.emulator.update_selection(col, row, side);
                cx.notify();
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.selecting {
                        this.selecting = false;
                        if !this.emulator.has_selection() {
                            this.emulator.clear_selection();
                        }
                        cx.notify();
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _, window, cx| {
                    this.focus_handle.focus(window, cx);
                    if this.copy_selection(cx) {
                        this.emulator.clear_selection();
                        cx.notify();
                    } else if this.interactive {
                        this.paste_clipboard(cx);
                    }
                    cx.stop_propagation();
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _, cx| {
                let m = this.metrics.get();
                let cw = m.cell_width.max(4.0);
                let lh = m.line_height.max(11.0);
                let max_scroll_x = (m.cols as f32 * cw - m.viewport_width).max(0.0);
                if max_scroll_x > 0.5 {
                    let raw_x_px = match event.delta {
                        ScrollDelta::Lines(delta) => delta.x * cw * 3.0,
                        ScrollDelta::Pixels(delta) => f32::from(delta.x),
                    };
                    let shift_y_px = if event.modifiers.shift && raw_x_px.abs() < 0.01 {
                        match event.delta {
                            ScrollDelta::Lines(delta) => delta.y * cw * 3.0,
                            ScrollDelta::Pixels(delta) => f32::from(delta.y),
                        }
                    } else {
                        0.0
                    };
                    let delta_x_px = if raw_x_px.abs() >= 0.01 {
                        raw_x_px
                    } else {
                        shift_y_px
                    };
                    if delta_x_px.abs() >= 0.01 {
                        this.scroll_x_px = (this.scroll_x_px - delta_x_px).clamp(0.0, max_scroll_x);
                        cx.stop_propagation();
                        cx.notify();
                        return;
                    }
                }
                let delta_lines = match event.delta {
                    ScrollDelta::Lines(delta) => delta.y.round() as i32,
                    ScrollDelta::Pixels(delta) => {
                        this.scroll_remainder_px += f32::from(delta.y);
                        let lines = (this.scroll_remainder_px / lh).trunc() as i32;
                        this.scroll_remainder_px -= lines as f32 * lh;
                        lines
                    }
                };
                if delta_lines == 0 {
                    return;
                }
                let mode = this.emulator.mode();
                if mode.contains(TermMode::ALT_SCREEN) && this.interactive {
                    let count = delta_lines.unsigned_abs().min(8);
                    let arrow = if delta_lines > 0 {
                        if mode.contains(TermMode::APP_CURSOR) {
                            b"\x1bOA"
                        } else {
                            b"\x1b[A"
                        }
                    } else if mode.contains(TermMode::APP_CURSOR) {
                        b"\x1bOB"
                    } else {
                        b"\x1b[B"
                    };
                    let mut seq = Vec::with_capacity(count as usize * arrow.len());
                    for _ in 0..count {
                        seq.extend_from_slice(arrow);
                    }
                    this.send_bytes(seq, cx);
                } else {
                    this.emulator.scroll_display(delta_lines);
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let key = event.keystroke.key.as_str();
                let modifiers = &event.keystroke.modifiers;

                if modifiers.control && modifiers.shift && key == "c" && this.copy_selection(cx) {
                    cx.stop_propagation();
                    return;
                }
                if modifiers.control
                    && !modifiers.shift
                    && !modifiers.alt
                    && key == "c"
                    && this.emulator.has_selection()
                    && this.copy_selection(cx)
                {
                    this.emulator.clear_selection();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                if !this.interactive {
                    return;
                }

                if modifiers.control && (key == "v" || (modifiers.shift && key == "v")) {
                    if this.paste_clipboard(cx) {
                        cx.stop_propagation();
                    }
                    return;
                }

                if modifiers.shift && matches!(key, "pageup" | "pagedown") {
                    let rows = this.metrics.get().rows.max(4) as i32;
                    let step = (rows / 2).max(1);
                    this.emulator
                        .scroll_display(if key == "pageup" { step } else { -step });
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }

                if let Some(bytes) = encode_keystroke(&event.keystroke, this.emulator.mode()) {
                    this.send_bytes(bytes, cx);
                    cx.stop_propagation();
                }
            }))
            .child(TerminalCanvasElement {
                view: entity,
                focus_handle: self.focus_handle.clone(),
                interactive,
            })
    }
}

struct ShapedTextAt {
    origin: Point<Pixels>,
    line: ShapedLine,
}

struct TerminalCanvasPrepaint {
    line_height: Pixels,
    bg_quads: Vec<PaintQuad>,
    text_runs: Vec<ShapedTextAt>,
    cursor_quads: Vec<PaintQuad>,
    cursor_text: Option<ShapedTextAt>,
    ime_quads: Vec<PaintQuad>,
    ime_text: Option<ShapedTextAt>,
    scrollbar_quad: Option<PaintQuad>,
    h_scrollbar_quad: Option<PaintQuad>,
}

struct TerminalCanvasElement {
    view: Entity<TerminalView>,
    focus_handle: FocusHandle,
    interactive: bool,
}

impl TerminalCanvasElement {
    fn mono_font(weight: FontWeight, style: FontStyle) -> Font {
        mono_font(weight, style)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RunStyleKey {
    fg_key: (u32, u32, u32, u32),
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strikeout: bool,
}

impl RunStyleKey {
    fn from_cell(fg: Hsla, flags: Flags) -> Self {
        Self {
            fg_key: (
                fg.h.to_bits(),
                fg.s.to_bits(),
                fg.l.to_bits(),
                fg.a.to_bits(),
            ),
            bold: flags.contains(Flags::BOLD),
            italic: flags.contains(Flags::ITALIC),
            underline: flags.intersects(Flags::ALL_UNDERLINES),
            undercurl: flags.contains(Flags::UNDERCURL),
            strikeout: flags.contains(Flags::STRIKEOUT),
        }
    }
}

fn build_text_run(len: usize, fg: Hsla, flags: Flags) -> TextRun {
    let weight = if flags.contains(Flags::BOLD) {
        FontWeight::BOLD
    } else {
        FontWeight::NORMAL
    };
    let style = if flags.contains(Flags::ITALIC) {
        FontStyle::Italic
    } else {
        FontStyle::Normal
    };
    let underline = flags
        .intersects(Flags::ALL_UNDERLINES)
        .then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(fg),
            wavy: flags.contains(Flags::UNDERCURL),
        });
    let strikethrough = flags
        .contains(Flags::STRIKEOUT)
        .then_some(StrikethroughStyle {
            thickness: px(1.0),
            color: Some(fg),
        });
    TextRun {
        len,
        font: TerminalCanvasElement::mono_font(weight, style),
        color: fg,
        background_color: None,
        underline,
        strikethrough,
    }
}

impl Element for TerminalCanvasElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalCanvasPrepaint;

    fn id(&self) -> Option<ElementId> {
        Some(ElementId::Name("terminal-canvas".into()))
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = LayoutStyle::default();
        style.flex_grow = 1.0;
        style.size.width = relative(1.0).into();
        style.size.height = relative(1.0).into();
        style.min_size.width = px(0.0).into();
        style.min_size.height = px(0.0).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let app_settings = settings::get(cx).clone();
        let ty = app_settings.typography;
        let t = *cx.colors();
        let term = terminal_palette(app_settings.appearance.terminal_scheme, &t);

        let font_override = self.view.read(cx).font_size_override;
        let font_size_f32 = font_override.unwrap_or(ty.code_font_size).clamp(8.5, 24.0);
        let font_size = px(font_size_f32);
        let line_height = px((font_size_f32 * 1.35).round()).max(px(11.0));
        let cw = measure_mono_cell_width(font_size_f32, window);
        let lh = f32::from(line_height);
        let ox = f32::from(bounds.origin.x);
        let oy = f32::from(bounds.origin.y);
        let viewport_w = f32::from(bounds.size.width).max(0.0);

        let (snapshot, marked_text, scroll_x_px, max_scroll_x, content_w) =
            self.view.update(cx, |view, cx| {
                let rows = ((f32::from(bounds.size.height) / lh).floor() as usize).clamp(4, 200);
                let can_resize = bounds.size.height >= px(48.0)
                    && (matches!(view.cols_mode, ColsMode::FixedCols(_))
                        || bounds.size.width >= px(120.0));
                if can_resize {
                    let cols = match view.cols_mode {
                        ColsMode::AutoFit => {
                            ((f32::from(bounds.size.width) / cw).floor() as usize).clamp(20, 500)
                        }
                        ColsMode::FixedCols(fixed) => fixed.clamp(20, 500),
                    };
                    if view.emulator.resize(cols, rows)
                        && let Some(on_resize) = view.on_resize.clone()
                    {
                        cx.defer(move |cx| {
                            on_resize(cols as u16, rows as u16, cx);
                        });
                    }
                }
                let snapshot = view.emulator.render_snapshot(&term);
                let content_w = snapshot.cols as f32 * cw;
                let max_scroll_x = (content_w - viewport_w).max(0.0);
                view.scroll_x_px = view.scroll_x_px.clamp(0.0, max_scroll_x);
                let scroll_x_px = view.scroll_x_px;
                let cursor_col =
                    (snapshot.cursor.point.column.0).min(snapshot.cols.saturating_sub(1));
                let cursor_row = (snapshot.cursor.point.line.0.max(0) as usize)
                    .min(snapshot.rows.saturating_sub(1));
                view.metrics.set(TerminalMetrics {
                    origin_x: ox,
                    origin_y: oy,
                    viewport_width: viewport_w,
                    scroll_x_px,
                    cell_width: cw,
                    line_height: lh,
                    cols: snapshot.cols,
                    rows: snapshot.rows,
                    cursor_col,
                    cursor_row,
                });
                (
                    snapshot,
                    view.marked_text.clone(),
                    scroll_x_px,
                    max_scroll_x,
                    content_w,
                )
            });
        let draw_ox = ox - scroll_x_px;

        // 1. 背景与选区矩形:同行相邻同色格合并为单个 Quad。
        let mut bg_quads = Vec::new();
        let mut active_bg: Option<(usize, usize, usize, Hsla)> = None;
        let flush_bg = |quads: &mut Vec<PaintQuad>, run: Option<(usize, usize, usize, Hsla)>| {
            if let Some((row, start_col, span, color)) = run {
                quads.push(fill(
                    Bounds::new(
                        point(px(draw_ox + start_col as f32 * cw), px(oy + row as f32 * lh)),
                        size(px(span as f32 * cw), line_height),
                    ),
                    color,
                ));
            }
        };
        for cell in &snapshot.cells {
            let effective_bg = if cell.selected {
                Some(term.selection_bg)
            } else {
                cell.bg
            };
            match (effective_bg, active_bg) {
                (Some(color), Some((r, start, span, prev_color)))
                    if r == cell.row && start + span == cell.col && prev_color == color =>
                {
                    active_bg = Some((r, start, span + 1, prev_color));
                }
                (Some(color), prev) => {
                    flush_bg(&mut bg_quads, prev);
                    active_bg = Some((cell.row, cell.col, 1, color));
                }
                (None, prev) => {
                    flush_bg(&mut bg_quads, prev);
                    active_bg = None;
                }
            }
        }
        flush_bg(&mut bg_quads, active_bg);

        // 2. 文本合批:单宽 ASCII 同样式连续合批;宽字符/非 ASCII 单格列锚定,绝不漂移。
        let mut text_runs = Vec::new();
        struct PendingAscii {
            row: usize,
            start_col: usize,
            next_col: usize,
            text: String,
            fg: Hsla,
            flags: Flags,
            key: RunStyleKey,
        }
        let mut pending: Option<PendingAscii> = None;
        let flush_ascii = |out: &mut Vec<ShapedTextAt>, item: Option<PendingAscii>| {
            if let Some(p) = item
                && !p.text.is_empty()
            {
                let run = build_text_run(p.text.len(), p.fg, p.flags);
                let shaped = window.text_system().shape_line(
                    SharedString::from(p.text),
                    font_size,
                    &[run],
                    None,
                );
                out.push(ShapedTextAt {
                    origin: point(px(draw_ox + p.start_col as f32 * cw), px(oy + p.row as f32 * lh)),
                    line: shaped,
                });
            }
        };

        let mut cursor_cell: Option<(char, Flags)> = None;
        let cursor_row_i32 = snapshot.cursor.point.line.0;
        let cursor_col_usize = snapshot.cursor.point.column.0;

        for cell in &snapshot.cells {
            if cell.row as i32 == cursor_row_i32 && cell.col == cursor_col_usize {
                cursor_cell = Some((cell.c, cell.flags));
            }
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                flush_ascii(&mut text_runs, pending.take());
                continue;
            }
            let has_decoration = cell
                .flags
                .intersects(Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
            if cell.c == ' ' && !has_decoration {
                flush_ascii(&mut text_runs, pending.take());
                continue;
            }
            let is_single_ascii = cell.c.is_ascii() && !cell.flags.contains(Flags::WIDE_CHAR);
            if is_single_ascii {
                let key = RunStyleKey::from_cell(cell.fg, cell.flags);
                match pending.as_mut() {
                    Some(p) if p.row == cell.row && p.next_col == cell.col && p.key == key => {
                        p.text.push(cell.c);
                        p.next_col += 1;
                    }
                    _ => {
                        flush_ascii(&mut text_runs, pending.take());
                        let mut text = String::with_capacity(16);
                        text.push(cell.c);
                        pending = Some(PendingAscii {
                            row: cell.row,
                            start_col: cell.col,
                            next_col: cell.col + 1,
                            text,
                            fg: cell.fg,
                            flags: cell.flags,
                            key,
                        });
                    }
                }
            } else {
                flush_ascii(&mut text_runs, pending.take());
                let mut text = String::with_capacity(4);
                text.push(cell.c);
                let run = build_text_run(text.len(), cell.fg, cell.flags);
                let shaped = window.text_system().shape_line(
                    SharedString::from(text),
                    font_size,
                    &[run],
                    None,
                );
                text_runs.push(ShapedTextAt {
                    origin: point(px(draw_ox + cell.col as f32 * cw), px(oy + cell.row as f32 * lh)),
                    line: shaped,
                });
            }
        }
        flush_ascii(&mut text_runs, pending.take());

        // 3. 光标与光标反色覆写字符。
        let mut cursor_quads = Vec::new();
        let mut cursor_text = None;
        let is_focused = self.focus_handle.is_focused(window);
        let cursor_visible = self.interactive
            && snapshot.display_offset == 0
            && snapshot.cursor.shape != CursorShape::Hidden
            && cursor_row_i32 >= 0
            && (cursor_row_i32 as usize) < snapshot.rows
            && cursor_col_usize < snapshot.cols;
        let cursor_origin = point(
            px(draw_ox + cursor_col_usize as f32 * cw),
            px(oy + cursor_row_i32.max(0) as f32 * lh),
        );
        if cursor_visible {
            let cursor_span = match cursor_cell {
                Some((_, flags)) if flags.contains(Flags::WIDE_CHAR) => 2.0,
                _ => 1.0,
            };
            let cursor_bounds = Bounds::new(cursor_origin, size(px(cw * cursor_span), line_height));
            let shape = if is_focused {
                snapshot.cursor.shape
            } else {
                CursorShape::HollowBlock
            };
            match shape {
                CursorShape::Block => {
                    cursor_quads.push(fill(cursor_bounds, term.cursor));
                    if let Some((ch, flags)) = cursor_cell
                        && ch != ' '
                        && !flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
                    {
                        let mut text = String::with_capacity(4);
                        text.push(ch);
                        let run = build_text_run(text.len(), term.cursor_text, flags);
                        let shaped = window.text_system().shape_line(
                            SharedString::from(text),
                            font_size,
                            &[run],
                            None,
                        );
                        cursor_text = Some(ShapedTextAt {
                            origin: cursor_origin,
                            line: shaped,
                        });
                    }
                }
                CursorShape::Beam => {
                    cursor_quads.push(fill(
                        Bounds::new(cursor_origin, size(px(2.0), line_height)),
                        term.cursor,
                    ));
                }
                CursorShape::Underline => {
                    cursor_quads.push(fill(
                        Bounds::new(
                            point(cursor_origin.x, cursor_origin.y + line_height - px(2.0)),
                            size(px(cw * cursor_span), px(2.0)),
                        ),
                        term.cursor,
                    ));
                }
                CursorShape::HollowBlock => {
                    cursor_quads.push(outline(cursor_bounds, term.cursor, BorderStyle::Solid));
                }
                CursorShape::Hidden => {}
            }
        }

        // 4. Windows IME 预编辑浮条(紧贴光标)。
        let mut ime_quads = Vec::new();
        let mut ime_text = None;
        if let Some(marked) = marked_text
            && !marked.is_empty()
            && self.interactive
        {
            let run = TextRun {
                len: marked.len(),
                font: Self::mono_font(FontWeight::NORMAL, FontStyle::Normal),
                color: t.text,
                background_color: None,
                underline: Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(t.accent),
                    wavy: false,
                }),
                strikethrough: None,
            };
            let shaped = window.text_system().shape_line(
                SharedString::from(marked),
                font_size,
                &[run],
                None,
            );
            let pill_w = shaped.width() + px(8.0);
            let max_x = f32::from(bounds.origin.x + bounds.size.width) - f32::from(pill_w);
            let pill_x = px(f32::from(cursor_origin.x).min(max_x).max(ox));
            let pill_bounds = Bounds::new(
                point(pill_x, cursor_origin.y),
                size(pill_w, line_height),
            );
            ime_quads.push(fill(pill_bounds, t.card));
            ime_quads.push(outline(pill_bounds, t.accent, BorderStyle::Solid));
            ime_text = Some(ShapedTextAt {
                origin: point(pill_x + px(4.0), cursor_origin.y),
                line: shaped,
            });
        }

        // 5. 右侧轻量回滚滑块(存在 scrollback 时才绘)。
        let scrollbar_quad = if snapshot.history_size > 0 && snapshot.rows > 0 {
            let total = (snapshot.history_size + snapshot.rows) as f32;
            let track_h = f32::from(bounds.size.height);
            let thumb_h = ((snapshot.rows as f32 / total) * track_h).clamp(18.0, track_h);
            let max_offset = snapshot.history_size as f32;
            let progress = if max_offset > 0.0 {
                1.0 - (snapshot.display_offset as f32 / max_offset).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let thumb_y = oy + (track_h - thumb_h) * progress;
            let thumb_alpha = if snapshot.display_offset > 0 { 0.48 } else { 0.16 };
            Some(fill(
                Bounds::new(
                    point(bounds.origin.x + bounds.size.width - px(5.0), px(thumb_y)),
                    size(px(3.0), px(thumb_h)),
                ),
                soft(term.fg, thumb_alpha),
            ))
        } else {
            None
        };

        // 6. 底部轻量横向滚动条(仅 FixedCols 视口窄于内容时绘)。
        let h_scrollbar_quad = if max_scroll_x > 1.0 && content_w > 0.0 {
            let track_w = viewport_w;
            let thumb_w = ((viewport_w / content_w) * track_w).clamp(24.0, track_w);
            let progress = (scroll_x_px / max_scroll_x).clamp(0.0, 1.0);
            let thumb_x = ox + (track_w - thumb_w) * progress;
            Some(fill(
                Bounds::new(
                    point(px(thumb_x), bounds.origin.y + bounds.size.height - px(4.0)),
                    size(px(thumb_w), px(3.0)),
                ),
                soft(term.fg, 0.28),
            ))
        } else {
            None
        };

        TerminalCanvasPrepaint {
            line_height,
            bg_quads,
            text_runs,
            cursor_quads,
            cursor_text,
            ime_quads,
            ime_text,
            scrollbar_quad,
            h_scrollbar_quad,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.interactive {
            window.handle_input(
                &self.focus_handle,
                ElementInputHandler::new(bounds, self.view.clone()),
                cx,
            );
        }

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for quad in prepaint.bg_quads.drain(..) {
                window.paint_quad(quad);
            }
            for item in &prepaint.text_runs {
                let _ = item.line.paint(
                    item.origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            for quad in prepaint.cursor_quads.drain(..) {
                window.paint_quad(quad);
            }
            if let Some(item) = &prepaint.cursor_text {
                let _ = item.line.paint(
                    item.origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            for quad in prepaint.ime_quads.drain(..) {
                window.paint_quad(quad);
            }
            if let Some(item) = &prepaint.ime_text {
                let _ = item.line.paint(
                    item.origin,
                    prepaint.line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            }
            if let Some(quad) = prepaint.scrollbar_quad.take() {
                window.paint_quad(quad);
            }
            if let Some(quad) = prepaint.h_scrollbar_quad.take() {
                window.paint_quad(quad);
            }
        });
    }
}
