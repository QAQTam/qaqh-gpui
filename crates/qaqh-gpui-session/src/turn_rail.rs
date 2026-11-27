//! 时间线左侧轮次导轨与悬停预览浮卡(Turn Minimap Rail & Preview Popover):
//! - 仅在对话轮次 >= 2 且鼠标靠近时间线左缘(或悬停在导轨/预览卡上)时经 `presence_overlay` 显现;
//! - 竖向圆点轨标示每轮用户提问与当前视口所在轮次;
//! - 悬停圆点弹出 L3 预览卡(提问摘要 / 助手回复摘要 / 轮次序号与耗时),点击即跳转对齐该轮起点。

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Context, FontWeight, ListOffset, MouseButton, SharedString, Window, div, px,
};
use qaqh_ringing_v3::{BlockActor, BlockKind, BlockView, InputPurpose};

use qaqh_gpui_kit::kit::{RADIUS_POPOVER, shadow_card, shadow_popover, soft};
use qaqh_gpui_kit::motion::{effective_motion, presence_overlay};
use qaqh_gpui_kit::settings::MotionLevel;
use qaqh_gpui_kit::theme::Palette;

use crate::store::SessionStore;
use crate::timeline::{TimelineItem, content_text};

const RAIL_PILL_W: f32 = 20.0;
const RAIL_MAX_H: f32 = 320.0;
const PREVIEW_CARD_W: f32 = 280.0;

struct TurnNavItem {
    display_index: usize,
    prompt: String,
    reply_preview: String,
    duration_label: Option<String>,
}

fn collapse_preview_text(raw: &str, max_chars: usize) -> String {
    let joined = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() <= max_chars {
        return joined;
    }
    let truncated: String = joined.chars().take(max_chars).collect();
    format!("{truncated}…")
}

fn format_duration_mmss(total_ms: u64) -> Option<String> {
    if total_ms == 0 {
        return None;
    }
    let total_secs = (total_ms + 500) / 1000;
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    Some(format!("{mins:02}:{secs:02}"))
}

fn collect_turns(store: &SessionStore, display_items: &[TimelineItem]) -> Vec<TurnNavItem> {
    let mut turn_starts: Vec<(usize, usize)> = Vec::new();
    for (display_idx, item) in display_items.iter().enumerate() {
        let TimelineItem::Single(entry_idx) = item else {
            continue;
        };
        if let Some(BlockView {
            kind: BlockKind::User,
            actor,
            purpose,
            ..
        }) = store.entries.get(*entry_idx)
            && *actor == Some(BlockActor::User)
            && *purpose == Some(InputPurpose::TriggerTurn)
        {
            turn_starts.push((*entry_idx, display_idx));
        }
    }

    if turn_starts.len() < 2 {
        return Vec::new();
    }

    let last_turn_idx = turn_starts.len() - 1;
    let mut result = Vec::with_capacity(turn_starts.len());

    for (idx, &(start_entry, display_index)) in turn_starts.iter().enumerate() {
        let end_entry = turn_starts
            .get(idx + 1)
            .map(|&(next_start, _)| next_start)
            .unwrap_or(store.entries.len());

        let (prompt, turn_id) = match store.entries.get(start_entry) {
            Some(BlockView {
                kind: BlockKind::User,
                content,
                images,
                attachments,
                turn_id,
                ..
            }) => {
                let text = collapse_preview_text(&content_text(store, content), 120);
                let prompt = if !text.is_empty() {
                    text
                } else if !images.is_empty() || !attachments.is_empty() {
                    "图片 / 附件提问".to_owned()
                } else {
                    "用户提问".to_owned()
                };
                (prompt, turn_id.as_ref())
            }
            _ => continue,
        };

        let mut answer_preview: Option<String> = None;
        let mut reasoning_preview: Option<String> = None;
        let mut total_ms: u64 = 0;

        for entry in store.entries.get(start_entry + 1..end_entry).into_iter().flatten() {
            let BlockView {
                kind,
                content,
                generated_ms,
                turn_id: block_turn_id,
                ..
            } = entry;
            if let Some(tid) = turn_id
                && block_turn_id.as_ref() != Some(tid)
            {
                continue;
            }
            if !matches!(kind, BlockKind::Answer | BlockKind::Reasoning) {
                continue;
            }
            if let Some(ms) = generated_ms {
                total_ms = total_ms.saturating_add(*ms);
            }
            let collapsed = collapse_preview_text(&content_text(store, content), 140);
            if collapsed.is_empty() {
                continue;
            }
            match kind {
                BlockKind::Answer if answer_preview.is_none() => {
                    answer_preview = Some(collapsed);
                }
                BlockKind::Reasoning if reasoning_preview.is_none() => {
                    reasoning_preview = Some(collapsed);
                }
                _ => {}
            }
        }

        if answer_preview.is_none() && idx == last_turn_idx {
            for (_, kind, text) in &store.streaming {
                let collapsed = collapse_preview_text(text, 140);
                if collapsed.is_empty() {
                    continue;
                }
                match kind {
                    BlockKind::Answer if answer_preview.is_none() => {
                        answer_preview = Some(collapsed);
                    }
                    BlockKind::Reasoning if reasoning_preview.is_none() => {
                        reasoning_preview = Some(collapsed);
                    }
                    _ => {}
                }
            }
        }

        let reply_preview = answer_preview
            .or(reasoning_preview)
            .unwrap_or_else(|| {
                if idx == last_turn_idx && store.current_turn_id.is_some() {
                    "回复生成中…".to_owned()
                } else {
                    "无正文摘要".to_owned()
                }
            });

        result.push(TurnNavItem {
            display_index,
            prompt,
            reply_preview,
            duration_label: format_duration_mmss(total_ms),
        });
    }

    result
}

fn active_turn_index(store: &SessionStore, turns: &[TurnNavItem]) -> usize {
    if turns.is_empty() {
        return 0;
    }
    if store.timeline_list.is_following_tail()
        || store.timeline_list.is_scrolled_to_end() == Some(true)
    {
        return turns.len() - 1;
    }
    let top_item_ix = store.timeline_list.logical_scroll_top().item_ix;
    turns
        .iter()
        .rposition(|turn| turn.display_index <= top_item_ix)
        .unwrap_or(0)
}

fn preview_card(
    store: &SessionStore,
    turns: &[TurnNavItem],
    t: &Palette,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
) -> Option<AnyElement> {
    let hovered_idx = store.turn_rail_hovered_idx.filter(|&i| i < turns.len());
    let sample = presence_overlay(
        "timeline-turn-preview",
        hovered_idx.is_some(),
        120,
        window,
        cx,
    );
    if !sample.should_render() {
        return None;
    }
    let display_idx = hovered_idx
        .or(store.turn_rail_last_idx)
        .filter(|&i| i < turns.len())?;
    let turn = &turns[display_idx];
    let target_display_index = turn.display_index;
    let total = turns.len();
    let p = sample.progress.clamp(0.0, 1.0);
    let shift_x = if effective_motion(cx) == MotionLevel::Full {
        (1.0 - p) * -6.0
    } else {
        0.0
    };

    let footer = div()
        .pt_1()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .text_xs()
        .text_color(t.faint)
        .child(SharedString::from(format!("{} / {}", display_idx + 1, total)))
        .children(turn.duration_label.as_ref().map(|dur| {
            div()
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_xs()
                .text_color(t.faint)
                .child(SharedString::from(dur.clone()))
        }));

    Some(
        div()
            .id("turn-rail-preview-card")
            .occlude()
            .ml(px(shift_x))
            .opacity(p)
            .w(px(PREVIEW_CARD_W))
            .p_3()
            .rounded(px(RADIUS_POPOVER))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .shadow(shadow_popover(t))
            .cursor_pointer()
            .flex()
            .flex_col()
            .gap_1p5()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |store, _, _, cx| {
                store.timeline_list.pause_following_tail();
                store.timeline_list.scroll_to(ListOffset {
                    item_ix: target_display_index,
                    offset_in_item: px(0.),
                });
                cx.notify();
            }))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .line_clamp(2)
                    .child(SharedString::from(turn.prompt.clone())),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(t.dim)
                    .line_clamp(2)
                    .child(SharedString::from(turn.reply_preview.clone())),
            )
            .child(footer)
            .into_any_element(),
    )
}

/// 渲染时间线左侧轮次导轨与悬停预览浮卡。
pub(crate) fn render(
    store: &SessionStore,
    display_items: &[TimelineItem],
    t: &Palette,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
) -> Option<AnyElement> {
    let turns = collect_turns(store, display_items);
    let visible = turns.len() >= 2
        && (store.turn_rail_near_left
            || store.turn_rail_hovered
            || store.turn_rail_hovered_idx.is_some());

    let sample = presence_overlay("timeline-turn-rail", visible, 160, window, cx);
    if !sample.should_render() || turns.len() < 2 {
        return None;
    }

    let p = sample.progress.clamp(0.0, 1.0);
    let shift_x = if effective_motion(cx) == MotionLevel::Full {
        (1.0 - p) * -8.0
    } else {
        0.0
    };
    let active_idx = active_turn_index(store, &turns);
    let hovered_idx = store.turn_rail_hovered_idx;

    let dots = turns.iter().enumerate().map(|(idx, turn)| {
        let is_active = idx == active_idx;
        let is_hovered = hovered_idx == Some(idx);
        let dot_size = if is_hovered {
            8.0
        } else if is_active {
            7.0
        } else {
            5.0
        };
        let dot_color = if is_hovered || is_active {
            t.text
        } else {
            soft(t.dim, 0.45)
        };
        let target_display_index = turn.display_index;

        div()
            .id(("turn-rail-dot", idx))
            .size(px(16.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .on_hover(cx.listener(move |store, hovered: &bool, _, cx| {
                if *hovered {
                    if store.turn_rail_hovered_idx != Some(idx) {
                        store.turn_rail_hovered_idx = Some(idx);
                        store.turn_rail_last_idx = Some(idx);
                        cx.notify();
                    }
                } else if store.turn_rail_hovered_idx == Some(idx) && !store.turn_rail_hovered {
                    store.turn_rail_hovered_idx = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |store, _, _, cx| {
                store.timeline_list.pause_following_tail();
                store.timeline_list.scroll_to(ListOffset {
                    item_ix: target_display_index,
                    offset_in_item: px(0.),
                });
                cx.notify();
            }))
            .child(div().size(px(dot_size)).rounded_full().bg(dot_color))
    });

    let rail_pill = div()
        .id("turn-rail-dots")
        .occlude()
        .w(px(RAIL_PILL_W))
        .max_h(px(RAIL_MAX_H))
        .overflow_y_scroll()
        .py_2()
        .px_0p5()
        .rounded_full()
        .bg(soft(t.card, 0.92))
        .border_1()
        .border_color(t.border_subtle)
        .shadow(shadow_card(t))
        .flex()
        .flex_col()
        .items_center()
        .gap_1()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(dots);

    let cluster = div()
        .id("turn-rail-cluster")
        .ml(px(shift_x))
        .opacity(p)
        .flex()
        .items_center()
        .gap_2()
        .on_hover(cx.listener(|store, hovered: &bool, _, cx| {
            let mut changed = store.turn_rail_hovered != *hovered;
            store.turn_rail_hovered = *hovered;
            if !*hovered && store.turn_rail_hovered_idx.is_some() {
                store.turn_rail_hovered_idx = None;
                changed = true;
            }
            if changed {
                cx.notify();
            }
        }))
        .child(rail_pill)
        .children(preview_card(store, &turns, t, window, cx));

    Some(
        div()
            .absolute()
            .left_2()
            .top_8()
            .bottom_8()
            .flex()
            .items_center()
            .child(cluster)
            .into_any_element(),
    )
}
