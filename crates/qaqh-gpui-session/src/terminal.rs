//! 无头虚拟终端内核(基于 `alacritty_terminal` 0.26,Apache-2.0)。
//!
//! 只使用 `Term<T>` + `vte::ansi::Processor` 做 VT100 / xterm / ConPTY 状态机模拟、
//! 回滚历史与鼠标选区,不创建本地 PTY;所有字节流与 resize 都由上层直接绑定到
//! `RingingV2` 的 WebSocket `ShellStream` 或只读 `exec_sessions` 捕获流。

use std::sync::{Arc, Mutex};

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::{
    Config, RenderableCursor, Term, TermMode, viewport_to_point,
};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor};
use gpui_kit::{Hsla, Keystroke, rgba};
use qaqh_gpui_kit::theme::TerminalPalette;

pub const DEFAULT_COLS: usize = 80;
pub const DEFAULT_ROWS: usize = 24;

const MIN_COLS: usize = 20;
const MAX_COLS: usize = 500;
const MIN_ROWS: usize = 4;
const MAX_ROWS: usize = 200;
const SCROLLBACK_HISTORY: usize = 4000;

/// 将终端主动应答(`PtyWrite`,如 ConPTY / PSReadLine 发起的 `CSI 6 n` DSR)
/// 与标题变更收集到内存队列,由上层在 `feed` 返回后立刻写回远端 PTY。
#[derive(Clone, Default)]
struct QueueListener {
    pty_writes: Arc<Mutex<Vec<Vec<u8>>>>,
    title: Arc<Mutex<Option<String>>>,
}

impl QueueListener {
    fn drain_writes(&self) -> Vec<Vec<u8>> {
        let mut guard = self
            .pty_writes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *guard)
    }

    fn title(&self) -> Option<String> {
        self.title
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn clear(&self) {
        self.pty_writes
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        *self
            .title
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }
}

impl EventListener for QueueListener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                self.pty_writes
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(text.into_bytes());
            }
            Event::Title(title) => {
                *self
                    .title
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(title);
            }
            Event::ResetTitle => {
                *self
                    .title
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
struct TermDimensions {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for TermDimensions {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }

    fn screen_lines(&self) -> usize {
        self.screen_lines
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

/// 视口内单个已解析栅格单元的渲染快照。
#[derive(Clone, Copy, Debug)]
pub struct RenderCell {
    pub col: usize,
    pub row: usize,
    pub c: char,
    pub fg: Hsla,
    pub bg: Option<Hsla>,
    pub flags: Flags,
    pub selected: bool,
}

/// 一帧可供 GPUI `Element` 直接合批绘制的视口快照。
pub struct TerminalRenderSnapshot {
    pub cols: usize,
    pub rows: usize,
    pub display_offset: usize,
    pub history_size: usize,
    pub cursor: RenderableCursor,
    pub cells: Vec<RenderCell>,
}

/// 无头虚拟终端封装:同时服务底坞交互 shell 与右侧只读执行面板。
pub struct TerminalEmulator {
    term: Term<QueueListener>,
    processor: Processor,
    listener: QueueListener,
    cols: usize,
    rows: usize,
    has_output: bool,
}

impl TerminalEmulator {
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = clamp_size(cols, rows);
        let listener = QueueListener::default();
        let term = build_term(cols, rows, listener.clone());
        Self {
            term,
            processor: Processor::new(),
            listener,
            cols,
            rows,
            has_output: false,
        }
    }

    /// 喂入 PTY 下行字节流,并返回必须回写远端 PTY 的应答序列(`CSI 6 n` 等)。
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        if bytes.is_empty() {
            return Vec::new();
        }
        self.has_output = true;
        self.processor.advance(&mut self.term, bytes);
        self.listener.drain_writes()
    }

    /// 动态调整列数与行数;尺寸确实变化时返回 `true` 以便同步 `ShellResize`。
    pub fn resize(&mut self, cols: usize, rows: usize) -> bool {
        let (cols, rows) = clamp_size(cols, rows);
        if self.cols == cols && self.rows == rows {
            return false;
        }
        self.term.resize(TermDimensions {
            columns: cols,
            screen_lines: rows,
        });
        self.cols = cols;
        self.rows = rows;
        true
    }

    /// 保持当前几何重置屏幕状态(用于清屏重建/切换执行会话)。
    pub fn reset(&mut self) {
        self.listener.clear();
        self.term = build_term(self.cols, self.rows, self.listener.clone());
        self.processor = Processor::new();
        self.has_output = false;
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols as u16, self.rows as u16)
    }

    pub fn has_output(&self) -> bool {
        self.has_output
    }

    pub fn mode(&self) -> TermMode {
        *self.term.mode()
    }

    pub fn display_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    pub fn title(&self) -> Option<String> {
        self.listener.title()
    }

    pub fn scroll_display(&mut self, delta: i32) {
        if delta != 0 {
            self.term.scroll_display(Scroll::Delta(delta));
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        if self.display_offset() != 0 {
            self.term.scroll_display(Scroll::Bottom);
        }
    }

    pub fn viewport_point(&self, col: usize, row: usize) -> Point {
        let col = col.min(self.cols.saturating_sub(1));
        let row = row.min(self.rows.saturating_sub(1));
        viewport_to_point(self.display_offset(), Point::new(row, Column(col)))
    }

    pub fn start_selection(&mut self, ty: SelectionType, col: usize, row: usize, side: Side) {
        let point = self.viewport_point(col, row);
        self.term.selection = Some(Selection::new(ty, point, side));
    }

    pub fn update_selection(&mut self, col: usize, row: usize, side: Side) {
        let point = self.viewport_point(col, row);
        if let Some(selection) = self.term.selection.as_mut() {
            selection.update(point, side);
        }
    }

    pub fn clear_selection(&mut self) {
        self.term.selection = None;
    }

    pub fn has_selection(&self) -> bool {
        self.term
            .selection
            .as_ref()
            .is_some_and(|selection| !selection.is_empty())
    }

    pub fn selection_text(&self) -> Option<String> {
        self.term
            .selection_to_string()
            .filter(|text| !text.is_empty())
    }

    /// 抽取当前可见视口单元、光标与选区范围。
    pub fn render_snapshot(&self, palette: &TerminalPalette) -> TerminalRenderSnapshot {
        let content = self.term.renderable_content();
        let display_offset = content.display_offset;
        let selection = content.selection;
        let cursor = content.cursor;
        let colors = content.colors;
        let mut cells = Vec::with_capacity(self.cols.saturating_mul(self.rows));

        for indexed in content.display_iter {
            let viewport_row = indexed.point.line.0 + display_offset as i32;
            if viewport_row < 0 || viewport_row >= self.rows as i32 {
                continue;
            }
            let selected = selection.is_some_and(|range| {
                range.contains_cell(&indexed, cursor.point, cursor.shape)
            });
            let (fg, bg) = resolve_cell_colors(
                indexed.cell.fg,
                indexed.cell.bg,
                indexed.cell.flags,
                colors,
                palette,
            );
            cells.push(RenderCell {
                col: indexed.point.column.0,
                row: viewport_row as usize,
                c: indexed.cell.c,
                fg,
                bg,
                flags: indexed.cell.flags,
                selected,
            });
        }

        TerminalRenderSnapshot {
            cols: self.cols,
            rows: self.rows,
            display_offset,
            history_size: self.history_size(),
            cursor,
            cells,
        }
    }
}

fn clamp_size(cols: usize, rows: usize) -> (usize, usize) {
    (cols.clamp(MIN_COLS, MAX_COLS), rows.clamp(MIN_ROWS, MAX_ROWS))
}

fn build_term(cols: usize, rows: usize, listener: QueueListener) -> Term<QueueListener> {
    let config = Config {
        scrolling_history: SCROLLBACK_HISTORY,
        ..Config::default()
    };
    let dimensions = TermDimensions {
        columns: cols,
        screen_lines: rows,
    };
    Term::new(config, &dimensions, listener)
}

fn rgba8_hsla(rgba8: u32) -> Hsla {
    Hsla::from(rgba(rgba8))
}

fn rgb_hsla(r: u8, g: u8, b: u8) -> Hsla {
    rgba8_hsla(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xFF)
}

fn xterm_256_hsla(idx: u8, colors: &Colors, palette: &TerminalPalette) -> Hsla {
    if let Some(rgb) = colors[idx as usize] {
        return rgb_hsla(rgb.r, rgb.g, rgb.b);
    }
    match idx {
        0..=15 => rgba8_hsla(palette.ansi16[idx as usize]),
        16..=231 => {
            const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
            let v = idx - 16;
            let r = CUBE[(v / 36) as usize];
            let g = CUBE[((v / 6) % 6) as usize];
            let b = CUBE[(v % 6) as usize];
            rgb_hsla(r, g, b)
        }
        232..=255 => {
            let level = 8 + 10 * (idx - 232);
            rgb_hsla(level, level, level)
        }
    }
}

fn resolve_named_fg(
    named: NamedColor,
    flags: Flags,
    colors: &Colors,
    palette: &TerminalPalette,
) -> Hsla {
    if let Some(rgb) = colors[named] {
        return rgb_hsla(rgb.r, rgb.g, rgb.b);
    }
    let dark_bg = palette.bg.l < 0.5;
    let bright_on_bold = dark_bg && flags.contains(Flags::BOLD);
    let dim_alpha = |mut c: Hsla| {
        c.a *= 0.65;
        c
    };
    match named {
        NamedColor::Black => rgba8_hsla(palette.ansi16[if bright_on_bold { 8 } else { 0 }]),
        NamedColor::Red => rgba8_hsla(palette.ansi16[if bright_on_bold { 9 } else { 1 }]),
        NamedColor::Green => rgba8_hsla(palette.ansi16[if bright_on_bold { 10 } else { 2 }]),
        NamedColor::Yellow => rgba8_hsla(palette.ansi16[if bright_on_bold { 11 } else { 3 }]),
        NamedColor::Blue => rgba8_hsla(palette.ansi16[if bright_on_bold { 12 } else { 4 }]),
        NamedColor::Magenta => rgba8_hsla(palette.ansi16[if bright_on_bold { 13 } else { 5 }]),
        NamedColor::Cyan => rgba8_hsla(palette.ansi16[if bright_on_bold { 14 } else { 6 }]),
        NamedColor::White => rgba8_hsla(palette.ansi16[if bright_on_bold { 15 } else { 7 }]),
        NamedColor::BrightBlack => rgba8_hsla(palette.ansi16[8]),
        NamedColor::BrightRed => rgba8_hsla(palette.ansi16[9]),
        NamedColor::BrightGreen => rgba8_hsla(palette.ansi16[10]),
        NamedColor::BrightYellow => rgba8_hsla(palette.ansi16[11]),
        NamedColor::BrightBlue => rgba8_hsla(palette.ansi16[12]),
        NamedColor::BrightMagenta => rgba8_hsla(palette.ansi16[13]),
        NamedColor::BrightCyan => rgba8_hsla(palette.ansi16[14]),
        NamedColor::BrightWhite => rgba8_hsla(palette.ansi16[15]),
        NamedColor::DimBlack => dim_alpha(rgba8_hsla(palette.ansi16[0])),
        NamedColor::DimRed => dim_alpha(rgba8_hsla(palette.ansi16[1])),
        NamedColor::DimGreen => dim_alpha(rgba8_hsla(palette.ansi16[2])),
        NamedColor::DimYellow => dim_alpha(rgba8_hsla(palette.ansi16[3])),
        NamedColor::DimBlue => dim_alpha(rgba8_hsla(palette.ansi16[4])),
        NamedColor::DimMagenta => dim_alpha(rgba8_hsla(palette.ansi16[5])),
        NamedColor::DimCyan => dim_alpha(rgba8_hsla(palette.ansi16[6])),
        NamedColor::DimWhite => dim_alpha(rgba8_hsla(palette.ansi16[7])),
        NamedColor::Foreground | NamedColor::BrightForeground => palette.fg,
        NamedColor::DimForeground => dim_alpha(palette.fg),
        NamedColor::Background => palette.bg,
        NamedColor::Cursor => palette.cursor,
    }
}

fn resolve_bg_color(color: Color, colors: &Colors, palette: &TerminalPalette) -> Option<Hsla> {
    match color {
        Color::Named(NamedColor::Background) => colors[NamedColor::Background].map(|rgb| rgb_hsla(rgb.r, rgb.g, rgb.b)),
        Color::Named(named) => Some(resolve_named_fg(named, Flags::empty(), colors, palette)),
        Color::Indexed(idx) => Some(xterm_256_hsla(idx, colors, palette)),
        Color::Spec(rgb) => Some(rgb_hsla(rgb.r, rgb.g, rgb.b)),
    }
}

fn resolve_cell_colors(
    fg_color: Color,
    bg_color: Color,
    flags: Flags,
    colors: &Colors,
    palette: &TerminalPalette,
) -> (Hsla, Option<Hsla>) {
    let mut fg = match fg_color {
        Color::Named(named) => resolve_named_fg(named, flags, colors, palette),
        Color::Indexed(idx) => xterm_256_hsla(idx, colors, palette),
        Color::Spec(rgb) => rgb_hsla(rgb.r, rgb.g, rgb.b),
    };
    let mut bg = resolve_bg_color(bg_color, colors, palette);
    if flags.contains(Flags::INVERSE) {
        let old_fg = fg;
        fg = bg.unwrap_or(palette.bg);
        bg = Some(old_fg);
    }
    if flags.contains(Flags::DIM) {
        fg.a *= 0.65;
    }
    if flags.contains(Flags::HIDDEN) {
        fg = bg.unwrap_or(palette.bg);
    }
    (fg, bg)
}

/// 将 GPUI `Keystroke` 编码为 PTY 控制字节。
///
/// Windows IME 关键规则:普通可打印字符与裸空格一律返回 `None`,交由 Windows
/// `TranslateMessage` → `WM_CHAR` / `WM_IME_COMPOSITION` → `EntityInputHandler::replace_text_in_range`
/// 唯一递交,避免与 `on_key_down` 双重上屏或截断中文输入法组合。
pub fn encode_keystroke(keystroke: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let key = keystroke.key.as_str();
    let modifiers = &keystroke.modifiers;
    if matches!(
        key,
        "control" | "alt" | "shift" | "platform" | "function" | "meta"
    ) {
        return None;
    }

    if modifiers.control && !modifiers.platform {
        return match key {
            k if k.len() == 1 => {
                let b = k.as_bytes()[0];
                if b.is_ascii_alphabetic() {
                    Some(vec![b.to_ascii_uppercase() - b'A' + 1])
                } else {
                    match b {
                        b'2' | b'@' => Some(vec![0x00]),
                        b'[' | b'3' => Some(vec![0x1b]),
                        b'\\' | b'4' => Some(vec![0x1c]),
                        b']' | b'5' => Some(vec![0x1d]),
                        b'6' | b'^' => Some(vec![0x1e]),
                        b'-' | b'_' | b'/' | b'7' => Some(vec![0x1f]),
                        b'8' => Some(vec![0x7f]),
                        _ => None,
                    }
                }
            }
            "space" => Some(vec![0x00]),
            "backspace" => Some(vec![0x08]),
            _ => encode_special_key(key, modifiers, mode),
        };
    }

    if modifiers.alt && !modifiers.platform {
        if key.len() == 1 {
            let b = key.as_bytes()[0];
            let out = if modifiers.shift && b.is_ascii_lowercase() {
                b.to_ascii_uppercase()
            } else {
                b
            };
            return Some(vec![0x1b, out]);
        }
        return match key {
            "space" => Some(b"\x1b ".to_vec()),
            "backspace" => Some(vec![0x1b, 0x7f]),
            "enter" => Some(b"\x1b\r".to_vec()),
            _ => encode_special_key(key, modifiers, mode),
        };
    }

    encode_special_key(key, modifiers, mode)
}

fn encode_special_key(
    key: &str,
    modifiers: &gpui_kit::Modifiers,
    mode: TermMode,
) -> Option<Vec<u8>> {
    let mod_param = 1
        + u8::from(modifiers.shift)
        + (u8::from(modifiers.alt) * 2)
        + (u8::from(modifiers.control) * 4);
    let has_mod = mod_param > 1;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);

    let cursor_key = |letter: char| -> Vec<u8> {
        if has_mod {
            format!("\x1b[1;{mod_param}{letter}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let tilde_key = |code: u8| -> Vec<u8> {
        if has_mod {
            format!("\x1b[{code};{mod_param}~").into_bytes()
        } else {
            format!("\x1b[{code}~").into_bytes()
        }
    };
    let ss3_or_mod_f = |letter: char| -> Vec<u8> {
        if has_mod {
            format!("\x1b[1;{mod_param}{letter}").into_bytes()
        } else {
            format!("\x1bO{letter}").into_bytes()
        }
    };

    Some(match key {
        "enter" => b"\r".to_vec(),
        "backspace" => vec![0x7f],
        "tab" => {
            if modifiers.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        "escape" => b"\x1b".to_vec(),
        "insert" => tilde_key(2),
        "delete" => tilde_key(3),
        "up" => cursor_key('A'),
        "down" => cursor_key('B'),
        "right" => cursor_key('C'),
        "left" => cursor_key('D'),
        "home" => cursor_key('H'),
        "end" => cursor_key('F'),
        "pageup" => tilde_key(5),
        "pagedown" => tilde_key(6),
        "f1" => ss3_or_mod_f('P'),
        "f2" => ss3_or_mod_f('Q'),
        "f3" => ss3_or_mod_f('R'),
        "f4" => ss3_or_mod_f('S'),
        "f5" => tilde_key(15),
        "f6" => tilde_key(17),
        "f7" => tilde_key(18),
        "f8" => tilde_key(19),
        "f9" => tilde_key(20),
        "f10" => tilde_key(21),
        "f11" => tilde_key(23),
        "f12" => tilde_key(24),
        _ => return None,
    })
}

/// 将剪贴板文本规范化为 PTY 粘贴字节(开启 `?2004h` 时自动包裹 Bracketed Paste)。
pub fn encode_paste(text: &str, mode: TermMode) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        let mut out = Vec::with_capacity(normalized.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(normalized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        normalized.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_palette() -> TerminalPalette {
        TerminalPalette {
            bg: rgba8_hsla(0x121417FF),
            fg: rgba8_hsla(0xE4E6EBFF),
            cursor: rgba8_hsla(0xE4E6EBFF),
            cursor_text: rgba8_hsla(0x121417FF),
            selection_bg: gpui_kit::hsla(0.58, 0.70, 0.60, 0.28),
            ansi16: [
                0x000000FF, 0xCD3131FF, 0x0DBC79FF, 0xE5E510FF, 0x2472C8FF, 0xBC3FBCFF,
                0x11A8CDFF, 0xE5E5E5FF, 0x666666FF, 0xF14C4CFF, 0x23D18BFF, 0xF5F543FF,
                0x3B8EEAFF, 0xD670D6FF, 0x29B8DBFF, 0xFFFFFFFF,
            ],
        }
    }

    #[test]
    fn renders_sgr_cursor_positioning_and_cjk_wide_chars() {
        let mut emu = TerminalEmulator::new(40, 10);
        emu.feed(b"\x1b[2;3H\x1b[31m\xE4\xB8\xAD\x1b[38;2;10;20;30mA");
        let snapshot = emu.render_snapshot(&test_palette());
        assert_eq!(snapshot.cursor.point.line.0, 1);
        assert_eq!(snapshot.cursor.point.column.0, 5);

        let cjk = snapshot
            .cells
            .iter()
            .find(|cell| cell.row == 1 && cell.col == 2)
            .expect("CJK leading cell exists");
        assert_eq!(cjk.c, '中');
        assert!(cjk.flags.contains(Flags::WIDE_CHAR));

        let spacer = snapshot
            .cells
            .iter()
            .find(|cell| cell.row == 1 && cell.col == 3)
            .expect("CJK spacer cell exists");
        assert!(spacer.flags.contains(Flags::WIDE_CHAR_SPACER));

        let ascii = snapshot
            .cells
            .iter()
            .find(|cell| cell.row == 1 && cell.col == 4)
            .expect("following ASCII cell exists");
        assert_eq!(ascii.c, 'A');
    }

    #[test]
    fn drains_dsr_pty_write_response() {
        let mut emu = TerminalEmulator::new(80, 24);
        let writes = emu.feed(b"\x1b[6n");
        assert_eq!(writes, vec![b"\x1b[1;1R".to_vec()]);
        assert!(emu.feed(b"ok").is_empty());
    }

    #[test]
    fn resize_reports_only_on_change() {
        let mut emu = TerminalEmulator::new(80, 24);
        assert!(!emu.resize(80, 24));
        assert!(emu.resize(120, 32));
        assert_eq!(emu.size(), (120, 32));
        assert!(!emu.resize(120, 32));
    }

    #[test]
    fn keystroke_and_paste_respect_ime_and_modes() {
        let plain_a = Keystroke::parse("a").expect("parse a");
        assert_eq!(encode_keystroke(&plain_a, TermMode::empty()), None);

        let ctrl_d = Keystroke::parse("ctrl-d").expect("parse ctrl-d");
        assert_eq!(encode_keystroke(&ctrl_d, TermMode::empty()), Some(vec![0x04]));

        let alt_b = Keystroke::parse("alt-b").expect("parse alt-b");
        assert_eq!(encode_keystroke(&alt_b, TermMode::empty()), Some(b"\x1bb".to_vec()));

        let up = Keystroke::parse("up").expect("parse up");
        assert_eq!(encode_keystroke(&up, TermMode::empty()), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode_keystroke(&up, TermMode::APP_CURSOR), Some(b"\x1bOA".to_vec()));

        assert_eq!(
            encode_paste("echo 1\necho 2", TermMode::empty()),
            b"echo 1\recho 2".to_vec()
        );
        assert_eq!(
            encode_paste("ls\r\npwd", TermMode::BRACKETED_PASTE),
            b"\x1b[200~ls\rpwd\x1b[201~".to_vec()
        );
    }
}
