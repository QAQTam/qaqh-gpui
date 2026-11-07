//! 文本输入核心(M2):TextField 缓冲 + EntityInputHandler + paint 期注册。
//!
//! 修复中文输入的根因:此前 composer/ask/rename 是裸 on_key_down 手搓,
//! 从未注册 InputHandler;gpui wayland 每帧 update_ime_enabled 只在有
//! input_handler 时才向合成器发 text_input.enable,合成器根本不知道
//! 本窗口要文本输入 → fcitx5 组字永远不启动。
//!
//! selection/marked 采用 UTF-16 偏移(IME 协议口径,InputHandler 文档
//! 明确 "Text offsets and ranges use UTF-16"),编辑时转 UTF-8 byte。

use std::ops::Range;

use gpui::App;
use gpui::AnyElement;
use gpui::Bounds;
use gpui::Context;
use gpui::Element;
use gpui::ElementId;
use gpui::ElementInputHandler;
use gpui::Entity;
use gpui::EntityInputHandler;
use gpui::FocusHandle;
use gpui::GlobalElementId;
use gpui::HighlightStyle;
use gpui::InspectorElementId;
use gpui::IntoElement;
use gpui::LayoutId;
use gpui::Pixels;
use gpui::StyledText;
use gpui::TextInputConfiguration;
use gpui::UTF16Selection;
use gpui::UnderlineStyle;
use gpui::Window;
use gpui::px;

use crate::store::DaemonStore;
use crate::store::SessionStore;

fn utf16_len(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// 单字段文本缓冲。
#[derive(Clone, Debug, Default)]
pub struct TextField {
    pub text: String,
    /// 选区(UTF-16):end 为光标位。
    selection: Range<usize>,
    /// IME 组字范围(UTF-16)。
    marked: Option<Range<usize>>,
}

impl TextField {
    pub fn with_text(text: &str) -> Self {
        let end = utf16_len(text);
        Self {
            text: text.into(),
            selection: end..end,
            marked: None,
        }
    }

    pub fn len_utf16(&self) -> usize {
        utf16_len(&self.text)
    }

    fn utf16_to_byte(&self, target: usize) -> usize {
        let mut utf16 = 0;
        let mut byte = 0;
        for ch in self.text.chars() {
            if utf16 >= target {
                break;
            }
            utf16 += ch.len_utf16();
            byte += ch.len_utf8();
        }
        byte
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.selection = 0..0;
        self.marked = None;
    }

    pub fn selected_text(&self) -> String {
        self.text.get(self.utf16_to_byte(self.selection.start)..self.utf16_to_byte(self.selection.end)).expect("selection is character aligned").to_string()
    }

    /// 用 text 替换给定范围(None = 组字或当前选区)并收拢光标。
    pub fn replace_range(&mut self, range: Option<Range<usize>>, text: &str) {
        let range = range
            .as_ref()
            .or(self.marked.as_ref())
            .unwrap_or(&self.selection);
        let (start_u16, end_u16) = (range.start, range.end);
        let (start, end) = (self.utf16_to_byte(start_u16), self.utf16_to_byte(end_u16));
        self.text.replace_range(start..end, text);
        let head = start_u16 + utf16_len(text);
        self.marked = None;
        self.selection = head..head;
    }

    pub fn insert(&mut self, text: &str) {
        self.replace_range(None, text);
    }

    /// IME 组字:替换 marked/选区并标记新范围(new_selected 相对新文本起点)。
    pub fn set_marked(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected: Option<Range<usize>>,
    ) {
        let range = range
            .as_ref()
            .or(self.marked.as_ref())
            .unwrap_or(&self.selection);
        let (start_u16, end_u16) = (range.start, range.end);
        let (start, end) = (self.utf16_to_byte(start_u16), self.utf16_to_byte(end_u16));
        self.text.replace_range(start..end, new_text);
        let inserted = utf16_len(new_text);
        let end_u16 = start_u16 + inserted;
        self.selection = new_selected
            .map(|sel| (start_u16 + sel.start)..(start_u16 + sel.end))
            .unwrap_or(end_u16..end_u16);
        self.marked = (!new_text.is_empty()).then_some(start_u16..end_u16);
    }

    pub fn unmark(&mut self) {
        self.marked = None;
    }

    pub fn delete_backward(&mut self) {
        if self.selection.start != self.selection.end {
            self.insert("");
            return;
        }
        let cursor_u16 = self.selection.end;
        let cursor = self.utf16_to_byte(cursor_u16);
        let Some((start, ch)) = self.text.split_at(cursor).0.char_indices().next_back() else {
            return;
        };
        self.text.replace_range(start..cursor, "");
        let head = cursor_u16 - ch.len_utf16();
        self.selection = head..head;
        self.marked = None;
    }

    pub fn delete_forward(&mut self) {
        if self.selection.start != self.selection.end {
            self.insert("");
            return;
        }
        let cursor_u16 = self.selection.end;
        let cursor = self.utf16_to_byte(cursor_u16);
        let Some(ch) = self.text.split_at(cursor).1.chars().next() else {
            return;
        };
        self.text.replace_range(cursor..cursor + ch.len_utf8(), "");
        self.selection = cursor_u16..cursor_u16;
        self.marked = None;
    }

    pub fn move_left(&mut self, extend: bool) {
        if !extend && self.selection.start != self.selection.end {
            let pos = self.selection.start;
            self.selection = pos..pos;
            return;
        }
        if self.selection.start == 0 {
            return;
        }
        let byte = self.utf16_to_byte(self.selection.start);
        let Some((_, ch)) = self.text.split_at(byte).0.char_indices().next_back() else {
            return;
        };
        let next = self.selection.start - ch.len_utf16();
        self.selection = next..if extend { self.selection.end } else { next };
    }

    pub fn move_right(&mut self, extend: bool) {
        if !extend && self.selection.start != self.selection.end {
            let pos = self.selection.end;
            self.selection = pos..pos;
            return;
        }
        if self.selection.end >= self.len_utf16() {
            return;
        }
        let byte = self.utf16_to_byte(self.selection.end);
        let Some(ch) = self.text.split_at(byte).1.chars().next() else {
            return;
        };
        let next = self.selection.end + ch.len_utf16();
        self.selection = if extend { self.selection.start } else { next }..next;
    }

    pub fn move_home(&mut self, extend: bool) {
        self.selection = if extend {
            0..self.selection.end
        } else {
            0..0
        };
    }

    pub fn move_end(&mut self, extend: bool) {
        let len = self.len_utf16();
        self.selection = if extend {
            self.selection.start.min(len)..len
        } else {
            len..len
        };
    }

    pub fn select_all(&mut self) {
        let len = self.len_utf16();
        self.selection = 0..len;
    }

    fn display_parts(&self, focused: bool) -> (String, Vec<Range<usize>>) {
        let cursor_byte = self.utf16_to_byte(self.selection.end);
        let caret = if focused { "▍" } else { "" };
        let (before, after) = self.text.split_at(cursor_byte);
        let display = [before, caret, after].concat();
        let mut ranges = Vec::new();
        if let Some(marked) = &self.marked {
            let start = self.utf16_to_byte(marked.start);
            let end = self.utf16_to_byte(marked.end);
            // Only the portion after the inserted caret moves in the display string.
            let split = if focused { cursor_byte.clamp(start, end) } else { end };
            if start < split {
                ranges.push(start..split);
            }
            if split < end {
                ranges.push(split + caret.len()..end + caret.len());
            }
        }
        (display, ranges)
    }

    /// 渲染内容:光标位插入 ▍(聚焦时),组字段加下划线。
    pub fn display(&self, focused: bool, window: &Window) -> StyledText {
        let (display, ranges) = self.display_parts(focused);
        let highlights = ranges.into_iter().map(|range| {
            let style = HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.),
                    color: None,
                    wavy: false,
                }),
                ..Default::default()
            };
            (range, style)
        });
        StyledText::new(display).with_default_highlights(&window.text_style(), highlights)
    }
}

#[cfg(test)]
mod tests {
    use super::TextField;

    #[test]
    fn ime_preedit_highlights_follow_utf8_boundaries_around_caret() {
        let mut field = TextField::with_text("前后");
        field.set_marked(Some(1..1), "a中😀", None);
        assert_eq!(field.display_parts(true), ("前a中😀▍后".into(), vec![3..11]));

        field.set_marked(None, "汉😀字", Some(3..3));
        assert_eq!(
            field.display_parts(true),
            ("前汉😀▍字后".into(), vec![3..10, 13..16]),
        );
        assert_eq!(field.display_parts(false), ("前汉😀字后".into(), vec![3..13]));

        field.replace_range(None, "中文");
        assert_eq!(field.display_parts(true), ("前中文▍后".into(), vec![]));
        let mut field = TextField::with_text("汉😀");
        field.move_left(false);
        field.insert("A");
        assert_eq!(field.text, "汉A😀");
        field.move_right(false);
        field.insert("B");
        assert_eq!(field.text, "汉A😀B");
    }
}

impl SessionStore {
    /// 当前持焦点的输入框(composer > rename > ask 自定义输入)。
    fn focused_field(&mut self, window: &Window) -> Option<&mut TextField> {
        if self.search_open && self.search_focus.is_focused(window) { return Some(&mut self.search); }
        if self.composer_focus.is_focused(window) {
            return Some(&mut self.composer);
        }
        if self.renaming.is_some() && self.rename_focus.is_focused(window) {
            return Some(self.renaming.as_mut().unwrap());
        }
        for (_, draft) in &mut self.ask_drafts {
            for index in 0..draft.customs.len() {
                if draft.focuses[index].is_focused(window) {
                    return Some(&mut draft.customs[index]);
                }
            }
        }
        None
    }
}

impl EntityInputHandler for SessionStore {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.focused_field(window)?;
        let (start, end) = (field.utf16_to_byte(range.start), field.utf16_to_byte(range.end));
        let _ = adjusted_range.replace(range);
        field.text.get(start..end).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.focused_field(window)?.selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        if self.search_open && self.search_focus.is_focused(window) { return self.search.marked.clone(); }
        if self.composer_focus.is_focused(window) {
            return self.composer.marked.clone();
        }
        if self.renaming.is_some() && self.rename_focus.is_focused(window) {
            return self.renaming.as_ref().unwrap().marked.clone();
        }
        for (_, draft) in &self.ask_drafts {
            for (index, field) in draft.customs.iter().enumerate() {
                if draft.focuses[index].is_focused(window) {
                    return field.marked.clone();
                }
            }
        }
        None
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_field(window) {
            field.unmark();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_field(window) {
            field.replace_range(range, text);
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_field(window) {
            field.set_marked(range, new_text, new_selected_range);
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // v1:候选窗贴输入框区域;精确字符坐标待接入文本 shape 度量。
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        // 鼠标定位到字符索引:暂不支持,光标只由键盘控制。
        None
    }

    fn text_input_configuration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> TextInputConfiguration {
        TextInputConfiguration::default()
    }
}

impl DaemonStore {
    fn focused_settings_field(&self, window: &Window) -> Option<&TextField> {
        if self.session_search_focus.is_focused(window) { return Some(&self.session_search); }
        if self.font_focus.is_focused(window) {
            Some(&self.font_field)
        } else {
            self.settings_form.as_ref()?.focused_field(window)
        }
    }

    fn focused_settings_field_mut(&mut self, window: &Window) -> Option<&mut TextField> {
        if self.session_search_focus.is_focused(window) { return Some(&mut self.session_search); }
        if self.font_focus.is_focused(window) {
            Some(&mut self.font_field)
        } else {
            self.settings_form.as_mut()?.focused_field_mut(window)
        }
    }
}

impl EntityInputHandler for DaemonStore {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let field = self.focused_settings_field(window)?;
        let (start, end) = (field.utf16_to_byte(range.start), field.utf16_to_byte(range.end));
        let _ = adjusted_range.replace(range);
        field.text.get(start..end).map(str::to_owned)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.focused_settings_field(window)?.selection.clone(),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.focused_settings_field(window)?.marked.clone()
    }

    fn unmark_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(field) = self.focused_settings_field_mut(window) {
            field.unmark();
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_settings_field_mut(window) {
            field.replace_range(range, text);
            cx.notify();
        }
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(field) = self.focused_settings_field_mut(window) {
            field.set_marked(range, new_text, new_selected_range);
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(element_bounds)
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }

    fn text_input_configuration(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> TextInputConfiguration {
        TextInputConfiguration::default()
    }
}

/// 输入框元素:包一层内部元素并在 paint 期把 EntityInputHandler 挂到窗口。
/// gpui 要求 handle_input 只能在 paint 阶段调用(带 debug_assert_paint)。
/// 泛型 V:SessionStore(composer/rename/ask)与 DaemonStore(设置页字体框)。
pub struct InputElement<V: EntityInputHandler> {
    focus: FocusHandle,
    entity: Entity<V>,
    inner: AnyElement,
}

impl<V: EntityInputHandler> InputElement<V> {
    pub fn new(entity: Entity<V>, focus: FocusHandle, inner: impl IntoElement) -> Self {
        Self {
            focus,
            entity,
            inner: inner.into_any_element(),
        }
    }
}

impl<V: EntityInputHandler> IntoElement for InputElement<V> {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl<V: EntityInputHandler> Element for InputElement<V> {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
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
        (self.inner.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.inner.paint(window, cx);
        window.handle_input(
            &self.focus,
            ElementInputHandler::new(bounds, self.entity.clone()),
            cx,
        );
    }
}
