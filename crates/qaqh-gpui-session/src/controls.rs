//! 时间线局部控制:圆角浮动消息搜索条(Ctrl+F 唤出)+ 对话全文抄本。
use gpui_kit::{Context, FontWeight, KeyDownEvent, SharedString, div, prelude::*, px};
use gpui_kit::component::input::Input;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{RADIUS_CHIP, RADIUS_POPOVER, shadow_popover};
use qaqh_gpui_kit::theme::ActiveTheme as _;

use crate::store::SessionStore;

/// 会话全文(输入 + assistant 块),「复制对话」用。
pub fn conversation_text(store: &SessionStore) -> String {
    store
        .entries
        .iter()
        .filter_map(|entry| match entry {
            qaqh_ringing_contract::state::TimelineEntry::Input { content, .. }
            | qaqh_ringing_contract::state::TimelineEntry::AssistantBlock { content, .. } => {
                Some(crate::timeline::content_text(content))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// 圆角浮动消息搜索条:唯一入口是 Ctrl+F(根层快捷键),Escape 或关闭按钮收起。
pub fn search(store: &SessionStore, cx: &mut Context<SessionStore>) -> Option<gpui_kit::AnyElement> {
    if !store.search_open {
        return None;
    }
    let t = *cx.colors();
    let query = store.search.read(cx).value().to_lowercase();
    let matches: Vec<_> = store
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let text = match entry {
                qaqh_ringing_contract::state::TimelineEntry::Input { content, .. }
                | qaqh_ringing_contract::state::TimelineEntry::AssistantBlock { content, .. } => {
                    crate::timeline::content_text(content)
                }
                _ => return None,
            };
            (!query.is_empty() && text.to_lowercase().contains(&query))
                .then_some((index, text.chars().take(72).collect::<String>()))
        })
        .collect();

    let count_badge = div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(if matches.is_empty() {
            t.surface_subtle
        } else {
            t.accent_soft
        })
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(if matches.is_empty() { t.dim } else { t.accent })
        .child(SharedString::from(format!("{} 处匹配", matches.len())));

    let header = div()
        .flex()
        .items_center()
        .gap_2()
        .child(Icon::new(IconName::Search).with_size(Size::XSmall).text_color(t.dim))
        .child(
            div()
                .id("message-search-input")
                .flex_1()
                .min_w_0()
                .on_key_down(cx.listener(|store, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        store.search_open = false;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .child(Input::new(&store.search).small()),
        )
        .child(count_badge)
        .child(
            div()
                .id("message-search-close")
                .size(px(24.))
                .rounded(px(RADIUS_CHIP))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(t.hover))
                .on_click(cx.listener(|store, _, _, cx| {
                    store.search_open = false;
                    cx.notify();
                }))
                .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.dim)),
        );

    let results = (!matches.is_empty()).then(|| {
        div()
            .id("search-results")
            .max_h(px(168.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_0p5()
            .pt_1p5()
            .border_t_1()
            .border_color(t.border_subtle)
            .children(matches.into_iter().map(|(entry_index, preview)| {
                let target_index = crate::timeline::display_index_for_entry(store, entry_index);
                div()
                    .id(("search-hit", entry_index))
                    .px_2()
                    .py_1()
                    .rounded(px(RADIUS_CHIP))
                    .text_xs()
                    .text_color(t.dim)
                    .truncate()
                    .cursor_pointer()
                    .hover(|style| style.bg(t.hover).text_color(t.text))
                    .child(preview)
                    .on_click(cx.listener(move |store, _, _, _| {
                        store.timeline_list.scroll_to_reveal_item(target_index)
                    }))
            }))
    });

    Some(
        div()
            .mx_4()
            .mt_3()
            .p_2p5()
            .rounded(px(RADIUS_POPOVER))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .shadow(shadow_popover(&t))
            .flex()
            .flex_col()
            .gap_2()
            .child(header)
            .children(results)
            .into_any_element(),
    )
}
