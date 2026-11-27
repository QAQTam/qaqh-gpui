//! 时间线渲染(DESIGN LAW v3.0 · v5.0.0):
//! - 纯净悬浮主舞台:去掉冗余会话头,专注叙事阅读与交互;
//! - Antigravity 无框轨迹脊柱(ToolBurst / Thinking):左侧 1px 脊柱 + 物理弹簧 MotionReveal;
//! - 中性扫光思考态 + 底部 20px 渐隐过渡 + 无空白常驻 footer。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Context, Div, Entity, FollowMode, Font,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, MouseMoveEvent, ParentElement,
    Pixels, Render, ScrollHandle, SharedString, StatefulInteractiveElement, Styled, TextRun, Window,
    div, list, px, relative, rems,
};
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_core_vocab::{
    ActivityState, BlockId, ContentRef, PermissionGrantScope, ToolCallId, TurnId,
};
use qaqh_ringing_v3::{
    BlockActor, BlockKind, BlockView, ChoiceTone, CommandInteractionResolution,
    ExecSessionStateView, InputPurpose, InteractionDecisionKind, InteractionKind, ToolCallStatus,
    ToolCallView,
};

use qaqh_gpui_kit::icons::{AppIcon, command_icon, render_app_icon};
use qaqh_gpui_kit::kit::{
    RADIUS_BUBBLE, RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, RADIUS_POPOVER, shadow_card,
    shadow_popover, soft,
};
use qaqh_gpui_kit::motion::{
    accordion_reveal, bottom_fade_mask, effective_motion, live_dot, presence_overlay,
    shimmer_label, spring_chevron, top_fade_mask,
};
use qaqh_gpui_kit::primitives::action_button;
use qaqh_gpui_kit::settings::{self, MESSAGE_MAX_WIDTH_PX, MotionLevel};
use qaqh_gpui_kit::theme::{ActiveTheme, Palette};

use crate::store::{ArgStream, SessionStore};

const TOOLTIP_PAD_X: f32 = 12.0;
const TOOLTIP_PAD_Y: f32 = 8.0;
const TOOLTIP_BORDER: f32 = 1.0;
const TOOLTIP_CHROME_W: f32 = (TOOLTIP_PAD_X + TOOLTIP_BORDER) * 2.0;
const TOOLTIP_EDGE_MARGIN: f32 = 16.0;
const TOOLTIP_MIN_INNER_W: f32 = 64.0;
const TOOLTIP_MIN_RIGHT_W: f32 = 280.0;
const TOOLTIP_MAX_OUTER_W: f32 = 680.0;
const TOOLTIP_MEASURE_SLACK: f32 = 4.0;

fn collapse_tooltip_text(raw: &str) -> SharedString {
    let joined = raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    SharedString::from(joined)
}

fn measure_unwrapped_width(
    text: &SharedString,
    font: Font,
    font_size: Pixels,
    line_height: Pixels,
    window: &Window,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let runs = [TextRun {
        len: text.len(),
        font,
        color: Hsla::default(),
        background_color: None,
        underline: None,
        strikethrough: None,
    }];
    match window
        .text_system()
        .shape_text(text.clone(), font_size, &runs, None, None)
    {
        Ok(lines) => lines
            .iter()
            .map(|line| f32::from(line.size(line_height).width))
            .fold(0.0_f32, f32::max)
            .ceil(),
        Err(_) => 0.0,
    }
}

/// 动态向右延伸、最多两行的悬停提示卡:用于 Todo 条目展示标题、说明与完成/更新依据。
struct WrappingTooltip {
    title: SharedString,
    description: Option<SharedString>,
    evidence: Option<SharedString>,
}

impl Render for WrappingTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.colors();
        let title = collapse_tooltip_text(self.title.as_ref());
        let description = self
            .description
            .as_deref()
            .map(collapse_tooltip_text)
            .filter(|s| !s.is_empty());
        let evidence = self
            .evidence
            .as_deref()
            .map(|ev| collapse_tooltip_text(&format!("原因：{ev}")))
            .filter(|s| !s.is_empty());

        let rem_size = window.rem_size();
        let font_size = rems(0.75).to_pixels(rem_size);
        let base_style = window.text_style();
        let line_height = base_style
            .line_height
            .to_pixels(font_size.into(), rem_size);
        let normal_font = base_style.font();
        let title_font = Font {
            weight: FontWeight::MEDIUM,
            ..normal_font.clone()
        };

        let mut natural_w =
            measure_unwrapped_width(&title, title_font, font_size, line_height, window);
        if let Some(desc) = &description {
            natural_w = natural_w.max(measure_unwrapped_width(
                desc,
                normal_font.clone(),
                font_size,
                line_height,
                window,
            ));
        }
        if let Some(ev) = &evidence {
            natural_w = natural_w.max(measure_unwrapped_width(
                ev,
                normal_font,
                font_size,
                line_height,
                window,
            ));
        }

        let viewport_w = f32::from(window.viewport_size().width);
        let mouse_x = f32::from(window.mouse_position().x);
        let viewport_cap = (viewport_w - TOOLTIP_EDGE_MARGIN).clamp(120.0, TOOLTIP_MAX_OUTER_W);
        let right_avail = (viewport_w - mouse_x - TOOLTIP_EDGE_MARGIN).max(0.0);
        let left_avail = (mouse_x - TOOLTIP_EDGE_MARGIN).max(0.0);
        let max_outer_w = if right_avail >= TOOLTIP_MIN_RIGHT_W {
            right_avail.min(viewport_cap)
        } else {
            right_avail
                .max(left_avail)
                .min(viewport_cap)
                .max(120.0_f32.min(viewport_cap))
        };
        let max_inner_w = (max_outer_w - TOOLTIP_CHROME_W).max(TOOLTIP_MIN_INNER_W);
        let inner_w = (natural_w + TOOLTIP_MEASURE_SLACK)
            .clamp(TOOLTIP_MIN_INNER_W.min(max_inner_w), max_inner_w);
        let outer_w = inner_w + TOOLTIP_CHROME_W;

        let has_detail = description.is_some() || evidence.is_some();
        let has_evidence = evidence.is_some();

        let title_el = if has_detail {
            div()
                .w(px(inner_w))
                .truncate()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text)
                .child(title)
        } else {
            div()
                .w(px(inner_w))
                .line_clamp(2)
                .text_ellipsis()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text)
                .child(title)
        };

        div()
            .w(px(outer_w))
            .px(px(TOOLTIP_PAD_X))
            .py(px(TOOLTIP_PAD_Y))
            .rounded(px(RADIUS_POPOVER))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .shadow(shadow_popover(&t))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap_1()
            .child(title_el)
            .children(description.map(|desc| {
                let row = div().w(px(inner_w)).text_xs().text_color(t.dim);
                if has_evidence {
                    row.truncate().child(desc)
                } else {
                    row.line_clamp(2).text_ellipsis().child(desc)
                }
            }))
            .children(evidence.map(|ev| {
                div()
                    .w(px(inner_w))
                    .pt_1()
                    .border_t_1()
                    .border_color(t.border_subtle)
                    .line_clamp(2)
                    .text_ellipsis()
                    .text_xs()
                    .text_color(t.dim)
                    .child(ev)
            }))
    }
}

fn todo_status_icon(anim_id: SharedString, status: &str, animate_motion: bool, t: &Palette) -> AnyElement {
    match status {
        "completed" => Icon::new(IconName::CircleCheck)
            .with_size(Size::XSmall)
            .text_color(t.ok)
            .into_any_element(),
        "cancelled" => Icon::new(IconName::CircleX)
            .with_size(Size::XSmall)
            .text_color(t.bad)
            .into_any_element(),
        "in_progress" => {
            let icon = Icon::new(IconName::LoaderCircle)
                .with_size(Size::XSmall)
                .text_color(t.accent);
            if animate_motion {
                icon.with_animation(
                    anim_id,
                    Animation::new(std::time::Duration::from_millis(900)).repeat(),
                    |icon, val| icon.rotate(gpui_kit::Radians(val * std::f32::consts::TAU)),
                )
                .into_any_element()
            } else {
                icon.into_any_element()
            }
        }
        _ => Icon::new(IconName::Circle)
            .with_size(Size::XSmall)
            .text_color(t.faint)
            .into_any_element(),
    }
}

fn todo_status_label(status: &str, t: &Palette) -> (&'static str, Hsla) {
    match status {
        "completed" => ("已完成", t.ok),
        "in_progress" => ("进行中", t.accent),
        "cancelled" => ("已取消", t.bad),
        _ => ("待办", t.dim),
    }
}

/// 内联悬挂于消息区右上角的圆角 Todo 卡片:
/// - 与可滚动消息列表并列定位(`absolute`),上下翻动消息区不影响此卡片;
/// - 右边距预留 18px 避让最右侧滚动条;
/// - 每条 Todo 严格单行省略号截断,鼠标悬停弹出自动换行完整标签;
/// - 顶部「任务」按钮与卡片右上角 × 均可随时收起/展开。
fn todo_floating_card(
    store: &SessionStore,
    cx: &mut Context<SessionStore>,
    t: &Palette,
) -> Option<AnyElement> {
    if !store.todo_card_open || store.todos.is_empty() {
        return None;
    }
    let total = store.todos.len();
    let done = store
        .todos
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    let ratio = done as f32 / total as f32;

    let header = div()
        .px_3()
        .pt_2p5()
        .pb_2()
        .flex()
        .flex_col()
        .gap_1p5()
        .border_b_1()
        .border_color(t.border_subtle)
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::ListChecks)
                                .with_size(Size::XSmall)
                                .text_color(if done == total { t.ok } else { t.accent }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child("任务"),
                        )
                        .child(
                            div()
                                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                                .text_xs()
                                .text_color(if done == total { t.ok } else { t.dim })
                                .child(SharedString::from(format!("{done}/{total}"))),
                        ),
                )
                .child(
                    div()
                        .id("close-floating-todo")
                        .size(px(20.))
                        .rounded(px(RADIUS_CHIP))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .tooltip(|window, cx| {
                            gpui_kit::component::tooltip::Tooltip::new("收起任务卡片")
                                .build(window, cx)
                        })
                        .on_click(cx.listener(|store, _, _, cx| {
                            store.todo_card_open = false;
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.dim)),
                ),
        )
        .child(
            div()
                .h(px(3.))
                .w_full()
                .rounded_full()
                .bg(t.surface_subtle)
                .overflow_hidden()
                .child(
                    div()
                        .h_full()
                        .w(relative(ratio))
                        .rounded_full()
                        .bg(if done == total { t.ok } else { t.accent }),
                ),
        );

    let mut list = div()
        .id("floating-todo-list")
        .max_h(px(220.))
        .overflow_y_scroll()
        .p_1p5()
        .flex()
        .flex_col()
        .gap_0p5();

    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    for (idx, item) in store.todos.iter().enumerate() {
        let in_progress = item.status == "in_progress";
        let settled = item.status == "completed" || item.status == "cancelled";
        let icon_el = todo_status_icon(
            SharedString::from(format!("floating-todo-spin-{idx}")),
            item.status.as_str(),
            animate_motion,
            t,
        );
        let full_title = SharedString::from(item.title.clone());
        let tip_desc = (!item.description.trim().is_empty())
            .then(|| SharedString::from(item.description.trim().to_owned()));
        let tip_evidence = item
            .evidence
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| SharedString::from(s.to_owned()));
        list = list.child(
            div()
                .id(SharedString::from(format!("floating-todo-row-{idx}")))
                .px_2()
                .py_1()
                .rounded(px(RADIUS_CHIP))
                .when(in_progress, |row| row.bg(t.accent_soft))
                .when(!in_progress, |row| row.hover(|s| s.bg(t.hover)))
                .flex()
                .items_center()
                .gap_2()
                .tooltip(move |_window, cx| {
                    let title = full_title.clone();
                    let description = tip_desc.clone();
                    let evidence = tip_evidence.clone();
                    cx.new(move |_| WrappingTooltip {
                        title,
                        description,
                        evidence,
                    })
                    .into()
                })
                .child(div().flex_shrink_0().child(icon_el))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .font_weight(if in_progress {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if settled { t.dim } else { t.text })
                        .child(item.title.clone()),
                ),
        );
    }

    Some(
        div()
            .id("floating-todo-card")
            .occlude()
            .absolute()
            .top(px(12.))
            .right(px(18.))
            .w(px(288.))
            .rounded(px(RADIUS_CARD))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .shadow(shadow_popover(t))
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(header)
            .child(list)
            .into_any_element(),
    )
}

/// 大内容未到货时的占位(blob 帧到达后重渲染)。
pub(crate) const CONTENT_LOADING: &str = "内容加载中…";

/// 内容引用 → 可渲染文本(blob 走 store 的字节缓存)。
pub(crate) fn content_text(store: &SessionStore, content: &ContentRef) -> String {
    store.content_text(content)
}

/// 视图投影条目:单条、连续成功工具归组(展开模式)、或限高过程轨迹视窗(折叠模式)。
pub(crate) enum TimelineItem {
    Single(usize),
    ToolBurst {
        group_id: usize,
        indices: Vec<usize>,
    },
    WorkTrace {
        trace_key: String,
        turn_id: Option<TurnId>,
        indices: Vec<usize>,
        attach_live_tail: bool,
    },
}

pub(crate) struct TimelineProjection {
    pub(crate) items: Vec<TimelineItem>,
    pub(crate) live_tail_attached: bool,
    pub(crate) live_tail_key: String,
    pub(crate) live_tail_turn_id: Option<TurnId>,
}

fn find_tool<'a>(store: &'a SessionStore, call_id: &ToolCallId) -> Option<&'a ToolCallView> {
    store.tools.iter().find(|tool| &tool.tool_call_id == call_id)
}

/// 工具参数 → JSON 对象(V3 参数是 `ContentRef`;大参数走 blob 时要等字节到货)。
fn tool_args(store: &SessionStore, tool: &ToolCallView) -> Option<serde_json::Value> {
    serde_json::from_str(&store.content_text(tool.args.as_ref()?)).ok()
}

fn is_trace_foldable_tool_view(store: &SessionStore, tool: &ToolCallView) -> bool {
    if crate::store::handoff_facts(tool, |content| store.content_text(content)).is_some()
        || todo_card_data(store, tool).is_some()
    {
        return false;
    }
    tool.status != ToolCallStatus::AwaitingInteraction
}

fn is_groupable_tool(store: &SessionStore, block: &BlockView) -> bool {
    if block.kind != BlockKind::ToolCall {
        return false;
    }
    let Some(call_id) = block.tool_call_id.as_ref() else {
        return false;
    };
    let Some(tool) = find_tool(store, call_id) else {
        return false;
    };
    if crate::store::handoff_facts(tool, |content| store.content_text(content)).is_some()
        || todo_card_data(store, tool).is_some()
    {
        return false;
    }
    tool.status == ToolCallStatus::Succeeded
}

fn has_live_answer_stream(store: &SessionStore) -> bool {
    store.streaming.iter().any(|(_, kind, text)| {
        *kind == BlockKind::Answer && has_assistant_text(*kind, text)
    })
}

fn has_live_foldable_tail(store: &SessionStore) -> bool {
    let has_live_reasoning = store.streaming.iter().any(|(_, kind, text)| {
        *kind != BlockKind::Answer && has_assistant_text(*kind, text)
    });
    if has_live_reasoning || !store.arg_stream.is_empty() {
        return true;
    }
    let referenced: HashSet<&ToolCallId> = store
        .entries
        .iter()
        .filter_map(|block| block.tool_call_id.as_ref())
        .collect();
    store
        .tools
        .iter()
        .any(|tool| {
            !referenced.contains(&tool.tool_call_id) && is_trace_foldable_tool_view(store, tool)
        })
}

fn entry_turn_id(block: &BlockView) -> Option<TurnId> {
    block.turn_id.clone()
}

fn is_user_turn_trigger(block: &BlockView) -> bool {
    block.kind == BlockKind::User
        && block.actor == Some(BlockActor::User)
        && block.purpose == Some(InputPurpose::TriggerTurn)
}

pub(crate) fn build_timeline_items(
    store: &SessionStore,
    collapse_work_trace: bool,
) -> TimelineProjection {
    let has_live_answer = has_live_answer_stream(store);
    if !collapse_work_trace {
        let mut items = Vec::new();
        let mut burst: Vec<usize> = Vec::new();
        let flush = |burst: &mut Vec<usize>, items: &mut Vec<TimelineItem>| {
            if burst.len() >= 2 {
                let group_id = burst[0];
                items.push(TimelineItem::ToolBurst {
                    group_id,
                    indices: std::mem::take(burst),
                });
            } else if let Some(single) = burst.pop() {
                items.push(TimelineItem::Single(single));
            }
        };

        for (index, entry) in store.entries.iter().enumerate() {
            if hidden_timeline_entry(store, entry) {
                continue;
            }
            if is_groupable_tool(store, entry) {
                burst.push(index);
            } else {
                flush(&mut burst, &mut items);
                items.push(TimelineItem::Single(index));
            }
        }
        flush(&mut burst, &mut items);
        return TimelineProjection {
            items,
            live_tail_attached: false,
            live_tail_key: format!("trace-{}", store.entries.len()),
            live_tail_turn_id: store.current_turn_id.clone(),
        };
    }

    // 按用户轮次分段:每轮仅保留最新一条非空 Answer 在主舞台,其余思考/工具/旧 Answer 归入 WorkTrace。
    let mut turn_bounds: Vec<usize> = store
        .entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| is_user_turn_trigger(entry).then_some(idx))
        .collect();
    if turn_bounds.first().copied() != Some(0) && !store.entries.is_empty() {
        turn_bounds.insert(0, 0);
    }

    let mut items = Vec::new();
    let mut last_non_foldable_anchor: usize = 0;

    for (turn_idx, &turn_start) in turn_bounds.iter().enumerate() {
        let turn_end = turn_bounds
            .get(turn_idx + 1)
            .copied()
            .unwrap_or(store.entries.len());
        let is_last_turn = turn_idx + 1 == turn_bounds.len();
        let turn_id = store.entries[turn_start..turn_end]
            .iter()
            .find_map(entry_turn_id);

        // 若本轮当前正在流式输出新的非空 Answer,则本轮所有已落库 Answer 均退回折叠区;
        // 否则仅本轮最后一条非空 Answer 留在主舞台。
        let promoted_answer_idx = if is_last_turn && has_live_answer {
            None
        } else {
            (turn_start..turn_end).rev().find(|&idx| {
                matches!(
                    store.entries.get(idx),
                    Some(BlockView {
                        kind: BlockKind::Answer,
                        content,
                        ..
                    }) if has_assistant_text(BlockKind::Answer, &content_text(store, content))
                )
            })
        };

        let mut trace_indices: Vec<usize> = Vec::new();
        let mut trace_anchor = last_non_foldable_anchor;
        let flush_trace = |trace_indices: &mut Vec<usize>,
                           items: &mut Vec<TimelineItem>,
                           anchor: usize,
                           turn_id: &Option<TurnId>| {
            if trace_indices.is_empty() {
                return;
            }
            items.push(TimelineItem::WorkTrace {
                trace_key: format!("trace-{anchor}"),
                turn_id: turn_id.clone(),
                indices: std::mem::take(trace_indices),
                attach_live_tail: false,
            });
        };

        for index in turn_start..turn_end {
            let Some(entry) = store.entries.get(index) else {
                continue;
            };
            if hidden_timeline_entry(store, entry) {
                continue;
            }
            let foldable = match entry.kind {
                BlockKind::ToolCall => entry
                    .tool_call_id
                    .as_ref()
                    .and_then(|call_id| find_tool(store, call_id))
                    .is_some_and(|tool| is_trace_foldable_tool_view(store, tool)),
                BlockKind::Reasoning => true,
                BlockKind::Answer => Some(index) != promoted_answer_idx,
                BlockKind::User | BlockKind::ToolResult | BlockKind::System => false,
            };

            if foldable {
                if trace_indices.is_empty() {
                    trace_anchor = last_non_foldable_anchor;
                }
                trace_indices.push(index);
            } else {
                flush_trace(&mut trace_indices, &mut items, trace_anchor, &turn_id);
                items.push(TimelineItem::Single(index));
                last_non_foldable_anchor = index + 1;
            }
        }
        flush_trace(&mut trace_indices, &mut items, trace_anchor, &turn_id);
    }

    let live_foldable = has_live_foldable_tail(store);
    let can_attach = (live_foldable || (store.is_running() && !has_live_answer))
        && matches!(items.last(), Some(TimelineItem::WorkTrace { .. }));
    let mut live_tail_attached = false;
    if can_attach && let Some(TimelineItem::WorkTrace { attach_live_tail, .. }) = items.last_mut() {
        *attach_live_tail = true;
        live_tail_attached = true;
    }

    TimelineProjection {
        items,
        live_tail_attached,
        live_tail_key: format!("trace-{last_non_foldable_anchor}"),
        live_tail_turn_id: store.current_turn_id.clone(),
    }
}

/// 搜索跳转索引映射:原始 entry 下标 → (投影列表项下标, 若位于折叠 WorkTrace 内则返回 trace_key)。
pub(crate) fn display_index_for_entry(
    store: &SessionStore,
    collapse_work_trace: bool,
    entry_index: usize,
) -> (usize, Option<String>) {
    let projection = build_timeline_items(store, collapse_work_trace);
    for (idx, item) in projection.items.iter().enumerate() {
        match item {
            TimelineItem::Single(index) if *index == entry_index => return (idx, None),
            TimelineItem::ToolBurst { indices, .. } if indices.contains(&entry_index) => {
                return (idx, None);
            }
            TimelineItem::WorkTrace {
                trace_key, indices, ..
            } if indices.contains(&entry_index) => {
                return (idx, Some(trace_key.clone()));
            }
            _ => {}
        }
    }
    (0, None)
}

/// 「复制对话」:跟随最后一条 assistant 输出,把当前时间线全文写进剪贴板。
fn copy_conversation_chip(entity: &Entity<SessionStore>, t: &Palette) -> AnyElement {
    let entity = entity.downgrade();
    div()
        .id("copy-conversation")
        .mt_1p5()
        .self_start()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .cursor_pointer()
        .text_xs()
        .text_color(t.dim)
        .hover(|style| style.bg(t.hover).text_color(t.text))
        .child(Icon::new(IconName::Copy).with_size(Size::XSmall).text_color(t.dim))
        .child("复制全文对话")
        .on_click(move |_, _, cx| {
            let _ = entity.update(cx, |store, cx| {
                let text = crate::controls::conversation_text(store);
                if !text.is_empty() {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                }
            });
        })
        .into_any_element()
}

fn state_label(status: ToolCallStatus) -> &'static str {
    match status {
        ToolCallStatus::Pending => "已就绪",
        ToolCallStatus::AwaitingInteraction => "等待确认",
        ToolCallStatus::Running => "执行中",
        ToolCallStatus::Cancelling => "取消中",
        ToolCallStatus::Succeeded => "已完成",
        ToolCallStatus::Failed => "失败",
        ToolCallStatus::Cancelled => "已取消",
        ToolCallStatus::Indeterminate => "结果未知",
    }
}

fn state_color(status: ToolCallStatus, t: &Palette) -> Hsla {
    match status {
        ToolCallStatus::Pending => t.dim,
        ToolCallStatus::AwaitingInteraction | ToolCallStatus::Cancelling => t.warn,
        ToolCallStatus::Running => t.accent,
        ToolCallStatus::Succeeded => t.ok,
        ToolCallStatus::Failed => t.bad,
        ToolCallStatus::Cancelled | ToolCallStatus::Indeterminate => t.dim,
    }
}

fn exec_state_label(state: &ExecSessionStateView, t: &Palette) -> (&'static str, Hsla) {
    match state {
        ExecSessionStateView::Running => ("运行中", t.accent),
        ExecSessionStateView::Stopping { .. } => ("停止中", t.warn),
        ExecSessionStateView::Finished { .. } => ("已结束", t.dim),
    }
}

/// 会话活动状态的语义色:运行 accent,等待/取消 warn,空闲 dim。
pub fn status_color(status: ActivityState, t: &Palette) -> Hsla {
    match status {
        ActivityState::Running => t.accent,
        ActivityState::WaitingUser | ActivityState::Cancelling => t.warn,
        ActivityState::Idle => t.dim,
    }
}

/// 侧栏状态点:运行/待响应/取消中高亮,空闲置灰。
pub fn status_dot(status: ActivityState, t: &Palette) -> Div {
    let color = status_color(status, t);
    let label = match status {
        ActivityState::Running => "运行中",
        ActivityState::WaitingUser => "待响应",
        ActivityState::Cancelling => "取消中",
        ActivityState::Idle => "空闲",
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().size_1p5().rounded_full().bg(color))
        .child(div().text_xs().text_color(color).child(label))
}

fn copy_context_menu(
    menu: PopupMenu,
    window: &mut Window,
    cx: &mut gpui_kit::App,
    message_text: &str,
    entity: &gpui_kit::WeakEntity<SessionStore>,
) -> PopupMenu {
    let selected = TextSelection::selected_text(window, cx);
    let selected_trimmed = selected.trim().to_string();
    let has_selection = !selected_trimmed.is_empty();
    let msg_owned = message_text.to_owned();
    let weak = entity.clone();
    menu.item(
        PopupMenuItem::new("复制选中内容")
            .icon(IconName::Copy)
            .disabled(!has_selection)
            .on_click(move |_, _, cx| {
                if !selected_trimmed.is_empty() {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                        selected_trimmed.clone(),
                    ));
                }
            }),
    )
    .item(
        PopupMenuItem::new("复制此条消息")
            .icon(IconName::Copy)
            .disabled(msg_owned.is_empty())
            .on_click(move |_, _, cx| {
                if !msg_owned.is_empty() {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(msg_owned.clone()));
                }
            }),
    )
    .separator()
    .item(
        PopupMenuItem::new("复制全文对话")
            .icon(IconName::Copy)
            .on_click(move |_, _, cx| {
                let _ = weak.update(cx, |store, cx| {
                    let text = crate::controls::conversation_text(store);
                    if !text.is_empty() {
                        cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                    }
                });
            }),
    )
}

fn user_bubble(
    id: SharedString,
    text: &str,
    entity: &Entity<SessionStore>,
    t: &Palette,
) -> AnyElement {
    let msg_text = text.to_owned();
    let weak = entity.downgrade();
    div()
        .id(id.clone())
        .self_end()
        .max_w(px(640.))
        .px_4()
        .py_2p5()
        .rounded(px(RADIUS_BUBBLE))
        .bg(t.bubble)
        .border_1()
        .border_color(soft(t.accent, 0.18))
        .text_sm()
        .text_color(t.text)
        .child(
            SelectableText::new(
                SharedString::from(format!("{id}-sel")),
                text.to_owned(),
            )
            .selection_color(t.selection_wash),
        )
        .context_menu(move |menu, window, cx| {
            copy_context_menu(menu, window, cx, &msg_text, &weak)
        })
        .into_any_element()
}

/// UTF-8 安全尾部截断。
fn utf8_tail(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let start = text.ceil_char_boundary(text.len() - max_bytes);
    (text.split_at(start).1, true)
}

fn tool_summary(name: &str, status: ToolCallStatus) -> Option<&'static str> {
    let (active, done) = match name {
        "todo_write" => ("正在创建待办", "已创建待办"),
        "todo_update" => ("正在更新待办", "已更新待办"),
        "todo_list" => ("正在查看待办", "已查看待办"),
        "ask" => ("正在询问用户", "用户已回答"),
        "read" => ("正在读取文件", "已读取文件"),
        "read_image" => ("正在查看图片", "已查看图片"),
        "glob" => ("正在查找文件", "已查找文件"),
        "grep" => ("正在搜索内容", "已搜索内容"),
        "edit" | "apply_patch" => ("正在编辑文件", "已编辑文件"),
        "write" => ("正在写入文件", "已写入文件"),
        "delete" => ("正在删除文件", "已删除文件"),
        "search" => ("正在搜索网页", "已搜索网页"),
        "web_fetch" => ("正在读取网页", "已读取网页"),
        "spawn_subagent" => ("正在派生子代理", "已派生子代理"),
        "send_message" => ("正在发送代理消息", "已发送代理消息"),
        "wait_agent" => ("正在等待子代理", "子代理已返回"),
        "list_agents" => ("正在查看代理列表", "已查看代理列表"),
        "interrupt_agent" => ("正在打断子代理", "已打断子代理"),
        "task_create" => ("正在创建任务", "已创建任务"),
        "task_claim" => ("正在认领任务", "已更新认领"),
        "task_update" => ("正在更新任务", "已更新任务"),
        "task_close" => ("正在关闭任务", "已关闭任务"),
        "task_list" => ("正在列出任务", "已列出任务"),
        "board_channel_create" => ("正在创建频道", "已创建频道"),
        "board_thread_create" => ("正在创建话题", "已创建话题"),
        "board_post" => ("正在留言", "已留言"),
        "board_subscribe" => ("正在更新订阅", "已更新订阅"),
        "board_list" => ("正在查看留言板", "已查看留言板"),
        "create_goal" => ("正在设定目标", "已设定目标"),
        "update_goal" => ("正在更新目标", "已更新目标"),
        "get_goal" => ("正在读取目标", "已读取目标"),
        _ => return None,
    };
    match status {
        ToolCallStatus::Succeeded => Some(done),
        ToolCallStatus::Pending | ToolCallStatus::Running => Some(active),
        ToolCallStatus::AwaitingInteraction if name == "ask" => Some(active),
        _ => None,
    }
}

fn tool_category_icon(name: &str, is_exec: bool) -> IconName {
    if is_exec {
        return IconName::Terminal;
    }
    match name {
        "read" | "read_image" => IconName::FileText,
        "edit" | "apply_patch" | "write" | "delete" => IconName::FilePen,
        "glob" | "grep" | "search" | "web_fetch" => IconName::Search,
        "todo_write" | "todo_update" | "todo_list" | "task_create" | "task_claim"
        | "task_update" | "task_close" | "task_list" => IconName::ListChecks,
        "spawn_subagent" | "send_message" | "wait_agent" | "list_agents"
        | "interrupt_agent" => IconName::Bot,
        _ => IconName::Wrench,
    }
}

fn summarize_burst(store: &SessionStore, indices: &[usize]) -> String {
    let (mut cmds, mut files, mut searches, mut todos, mut agents, mut others) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    for &idx in indices {
        let Some(call_id) = store
            .entries
            .get(idx)
            .filter(|block| block.kind == BlockKind::ToolCall)
            .and_then(|block| block.tool_call_id.as_ref())
        else {
            continue;
        };
        let Some(tool) = store.tools.iter().find(|t| &t.tool_call_id == call_id) else {
            others += 1;
            continue;
        };
        let name = tool.tool_name.as_str();
        let is_exec = name == "exec_command"
            || store.exec_sessions.iter().any(|e| e.origin_call_id.as_ref() == Some(&tool.tool_call_id));
        if is_exec {
            cmds += 1;
        } else if matches!(name, "edit" | "apply_patch" | "write" | "delete") {
            files += 1;
        } else if matches!(name, "read" | "read_image" | "glob" | "grep" | "search" | "web_fetch") {
            searches += 1;
        } else if matches!(name, "todo_write" | "todo_update" | "todo_list") {
            todos += 1;
        } else if name.starts_with("spawn_")
            || name.ends_with("_agent")
            || name.starts_with("task_")
            || name.starts_with("board_")
        {
            agents += 1;
        } else {
            others += 1;
        }
    }
    let mut parts = Vec::new();
    if cmds > 0 {
        parts.push(format!("命令 ×{cmds}"));
    }
    if files > 0 {
        parts.push(format!("改写 ×{files}"));
    }
    if searches > 0 {
        parts.push(format!("查阅 ×{searches}"));
    }
    if todos > 0 {
        parts.push(format!("待办 ×{todos}"));
    }
    if agents > 0 {
        parts.push(format!("协作 ×{agents}"));
    }
    if others > 0 {
        parts.push(format!("工具 ×{others}"));
    }
    if parts.is_empty() {
        format!("已完成 {} 项操作", indices.len())
    } else {
        format!("已完成 {} 项操作 · {}", indices.len(), parts.join(" · "))
    }
}

fn tool_status_only(name: &str) -> bool {
    matches!(
        name,
        "ask"
            | "todo_write"
            | "todo_update"
            | "todo_list"
            | "task_list"
            | "board_list"
            | "list_agents"
            | "get_goal"
    )
}

fn tool_status_line(name: &str, title: String, status: ToolCallStatus, t: &Palette) -> Div {
    let icon = tool_category_icon(name, false);
    let color = state_color(status, t);
    div()
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .text_color(t.dim)
        .child(Icon::new(icon).with_size(Size::XSmall).text_color(color))
        .child(div().min_w_0().truncate().child(SharedString::from(title)))
}

/// 折叠行状态 icon:loader(运行时旋转)/圈对/圈叹号。
fn tool_status_icon(call_id: &ToolCallId, status: ToolCallStatus, t: &Palette) -> AnyElement {
    let (name, spin) = match status {
        ToolCallStatus::Succeeded => (IconName::CircleCheck, false),
        ToolCallStatus::Failed | ToolCallStatus::Cancelled => (IconName::CircleAlert, false),
        ToolCallStatus::Pending => (IconName::LoaderCircle, false),
        _ => (IconName::LoaderCircle, true),
    };
    let icon = Icon::new(name)
        .with_size(Size::XSmall)
        .text_color(state_color(status, t));
    if spin {
        icon.with_animation(
            SharedString::from(format!("tool-spin-{}", call_id.as_str())),
            Animation::new(std::time::Duration::from_millis(900)).repeat(),
            |icon, value| icon.rotate(gpui_kit::Radians(value * std::f32::consts::TAU)),
        )
        .into_any_element()
    } else {
        icon.into_any_element()
    }
}

fn tool_exec_icon(
    name: &str,
    is_exec: bool,
    command: Option<&str>,
    shell: Option<&str>,
) -> AppIcon {
    if is_exec {
        command_icon(command, shell).unwrap_or(AppIcon::Lucide(IconName::Terminal))
    } else {
        AppIcon::Lucide(tool_category_icon(name, false))
    }
}

#[allow(clippy::too_many_arguments)]
fn tool_toggle(
    entity: &Entity<SessionStore>,
    call_id: &ToolCallId,
    expanded: bool,
    status: AnyElement,
    category: AppIcon,
    title: String,
    delta: Option<(usize, usize)>,
    animate_motion: bool,
    t: &Palette,
) -> gpui_kit::Stateful<Div> {
    let entity = entity.downgrade();
    let call_id = call_id.clone();
    div()
        .id(SharedString::from(format!("tool-toggle-{}", call_id.as_str())))
        .flex()
        .items_center()
        .gap_2()
        .px_2p5()
        .py_1p5()
        .rounded(px(RADIUS_CONTROL))
        .text_xs()
        .text_color(t.dim)
        .cursor_pointer()
        .hover(|style| style.bg(t.hover).text_color(t.text))
        .child(div().flex_shrink_0().child(status))
        .child(render_app_icon(category, 14.0, t.dim))
        .child(div().flex_1().min_w_0().truncate().child(SharedString::from(title)))
        .children(delta.filter(|&(a, r)| a > 0 || r > 0).map(|(added, removed)| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .px_1p5()
                .py(px(1.))
                .rounded(px(RADIUS_CHIP))
                .bg(t.surface_subtle)
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_xs()
                .child(div().text_color(t.ok).child(format!("+{added}")))
                .child(div().text_color(t.bad).child(format!("−{removed}")))
        }))
        .child(spring_chevron(
            SharedString::from(format!("tool-chev-{}", call_id.as_str())),
            expanded,
            t.faint,
            animate_motion,
        ))
        .on_click(move |_, _, cx| {
            let _ = entity.update(cx, |store, cx| {
                if !store.expanded_tools.remove(&call_id) {
                    store.expanded_tools.insert(call_id.clone());
                }
                cx.notify();
            });
        })
}

/// spawn_subagent 的 fork 徽标后缀。
fn fork_suffix(name: &str, args: Option<&serde_json::Value>) -> &'static str {
    let is_fork = name == "spawn_subagent"
        && args
            .and_then(|args| args.get("mode"))
            .and_then(|mode| mode.as_str())
            == Some("fork");
    if is_fork { " · 继承历史" } else { "" }
}

/// 参数标量摘要:key=value 拼接,字符串值截 72 字。
fn args_summary(args: &serde_json::Value) -> Option<String> {
    let object = args.as_object()?;
    let parts: Vec<String> = object
        .iter()
        .filter_map(|(key, value)| {
            let rendered = match value {
                serde_json::Value::String(text) => {
                    let text = text.trim();
                    if text.is_empty() {
                        return None;
                    }
                    let capped: String = text.chars().take(72).collect();
                    format!(
                        "{capped}{}",
                        if text.chars().count() > 72 { "…" } else { "" }
                    )
                }
                serde_json::Value::Number(number) => number.to_string(),
                serde_json::Value::Bool(flag) => flag.to_string(),
                _ => return None,
            };
            Some(format!("{key}={rendered}"))
        })
        .collect();
    (!parts.is_empty()).then(|| parts.join("  "))
}

const MAX_TODO_CARD_ITEMS: usize = 50;

struct TodoCardItem {
    title: String,
    status: &'static str,
    description: Option<String>,
    evidence: Option<String>,
}

enum TodoCardData {
    List {
        items: Vec<TodoCardItem>,
        omitted: usize,
    },
    Update {
        title: String,
        status: &'static str,
        evidence: Option<String>,
    },
}

fn normalize_todo_status(raw: Option<&str>) -> &'static str {
    match raw.unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "in_progress" | "in-progress" | "active" | "working" => "in_progress",
        "completed" | "done" => "completed",
        "cancelled" | "canceled" => "cancelled",
        _ => "pending",
    }
}

fn parse_todo_list_text(text: &str) -> Option<Vec<TodoCardItem>> {
    let mut items = Vec::new();
    for raw in text.lines() {
        let mut body = raw.trim();
        if body.is_empty() {
            continue;
        }
        // 弹头 `- ` 仅在后随空白时剥离,`[-]` 取消标记自身以 `-` 开头。
        if let Some(stripped) = body.strip_prefix("- ") {
            body = stripped.trim_start();
        }
        if !body.starts_with('[') {
            return None;
        }
        let close = body.find(']')?;
        let marker = &body[1..close];
        let status = match marker {
            " " => "pending",
            "~" => "in_progress",
            "x" | "X" => "completed",
            "-" => "cancelled",
            _ => return None,
        };
        let rest = body[close + 1..].trim();
        if rest.is_empty() {
            return None;
        }
        let fields: Vec<&str> = rest.split('|').collect();
        if fields.len() > 3 {
            return None;
        }
        let title = fields[0].trim();
        if title.is_empty() {
            return None;
        }
        let description = fields
            .get(1)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        let evidence = fields
            .get(2)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        items.push(TodoCardItem {
            title: title.to_owned(),
            status,
            description,
            evidence,
        });
    }
    Some(items)
}

fn parse_todo_card_item(raw: &serde_json::Value) -> Option<TodoCardItem> {
    let obj = raw.as_object()?;
    let title = obj.get("title")?.as_str()?.trim();
    if title.is_empty() {
        return None;
    }
    let description = obj
        .get("description")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    let evidence = obj
        .get("evidence")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    Some(TodoCardItem {
        title: title.to_owned(),
        status: normalize_todo_status(obj.get("status").and_then(|v| v.as_str())),
        description,
        evidence,
    })
}

/// 仅当 todo 写操作处于成功终态且入参可解析时返回时间线原位快照卡片数据;
/// 只读 `todo_list` / 非成功终态 / 解析失败一律返回 `None` 回退普通状态行。
fn todo_card_data(store: &SessionStore, tool: &ToolCallView) -> Option<TodoCardData> {
    let name = tool.tool_name.as_str();
    if !matches!(name, "todo" | "todo_write" | "todo_create" | "todo_update") {
        return None;
    }
    if tool.status != ToolCallStatus::Succeeded {
        return None;
    }
    let args = tool_args(store, tool)?;
    let args = args.as_object()?;
    if let Some(list_text) = args.get("list").and_then(|v| v.as_str()) {
        let parsed = parse_todo_list_text(list_text)?;
        if parsed.is_empty() {
            return None;
        }
        let omitted = parsed.len().saturating_sub(MAX_TODO_CARD_ITEMS);
        let items = parsed.into_iter().take(MAX_TODO_CARD_ITEMS).collect();
        return Some(TodoCardData::List { items, omitted });
    }
    let raw_items = args
        .get("items")
        .and_then(|v| v.as_array())
        .or_else(|| args.get("todos").and_then(|v| v.as_array()));
    if let Some(raw_items) = raw_items {
        let parsed: Vec<TodoCardItem> = raw_items.iter().filter_map(parse_todo_card_item).collect();
        if parsed.is_empty() {
            return None;
        }
        let omitted = parsed.len().saturating_sub(MAX_TODO_CARD_ITEMS);
        let items = parsed.into_iter().take(MAX_TODO_CARD_ITEMS).collect();
        return Some(TodoCardData::List { items, omitted });
    }
    let title = args
        .get("title")
        .or_else(|| args.get("id"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let evidence = args
        .get("evidence")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);
    Some(TodoCardData::Update {
        title: title.to_owned(),
        status: normalize_todo_status(args.get("status").and_then(|v| v.as_str())),
        evidence,
    })
}

fn render_inline_todo_card(
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    tool: &ToolCallView,
    name: &str,
    data: TodoCardData,
    animate_motion: bool,
    t: &Palette,
) -> Div {
    let action = if name == "todo_create" { "已创建" } else { "已更新" };
    match data {
        TodoCardData::Update {
            title,
            status,
            evidence,
        } => {
            let (status_label, status_color) = todo_status_label(status, t);
            let icon_el = todo_status_icon(
                SharedString::from(format!("todo-inline-spin-{}", tool.tool_call_id.as_str())),
                status,
                animate_motion,
                t,
            );
            div()
                .rounded(px(RADIUS_CARD))
                .border_1()
                .border_color(t.border_subtle)
                .bg(t.card)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px_3()
                        .py_1p5()
                        .bg(t.surface_subtle)
                        .border_b_1()
                        .border_color(t.border_subtle)
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(
                                    Icon::new(IconName::ListChecks)
                                        .with_size(Size::XSmall)
                                        .text_color(t.dim),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text)
                                        .child("待办"),
                                )
                                .child(div().text_xs().text_color(t.dim).child(action)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(status_color)
                                .child(status_label),
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(div().pt(px(2.)).flex_shrink_0().child(icon_el))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap_0p5()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.text)
                                        .child(SharedString::from(title)),
                                )
                                .children(evidence.map(|ev| {
                                    div()
                                        .text_xs()
                                        .text_color(t.dim)
                                        .child(SharedString::from(format!("原因：{ev}")))
                                })),
                        ),
                )
        }
        TodoCardData::List { items, omitted } => {
            let total = items.len();
            let done = items.iter().filter(|i| i.status == "completed").count();
            let ratio = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            let is_open = (total <= 12) ^ store.toggled_todo_cards.contains(&tool.tool_call_id);
            let call_id = tool.tool_call_id.clone();
            let toggle_entity = entity.clone();

            let header = div()
                .id(SharedString::from(format!("todo-inline-toggle-{}", tool.tool_call_id.as_str())))
                .px_3()
                .py_1p5()
                .bg(t.surface_subtle)
                .when(is_open, |el| el.border_b_1().border_color(t.border_subtle))
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(move |_, _, cx| {
                    let call_id = call_id.clone();
                    let _ = toggle_entity.update(cx, |store, cx| {
                        if !store.toggled_todo_cards.remove(&call_id) {
                            store.toggled_todo_cards.insert(call_id);
                        }
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            Icon::new(IconName::ListChecks)
                                .with_size(Size::XSmall)
                                .text_color(if done == total { t.ok } else { t.dim }),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child("待办"),
                        )
                        .child(div().text_xs().text_color(t.dim).child(action)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                                .text_xs()
                                .text_color(if done == total { t.ok } else { t.dim })
                                .child(SharedString::from(format!("{done}/{total}"))),
                        )
                        .child(
                            Icon::new(if is_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .with_size(Size::XSmall)
                            .text_color(t.dim),
                        ),
                );

            let mut card = div()
                .rounded(px(RADIUS_CARD))
                .border_1()
                .border_color(t.border_subtle)
                .bg(t.card)
                .overflow_hidden()
                .flex()
                .flex_col()
                .child(header);

            if is_open {
                let mut list = div()
                    .id(SharedString::from(format!("todo-inline-list-{}", tool.tool_call_id.as_str())))
                    .max_h(px(264.))
                    .overflow_y_scroll()
                    .px_2()
                    .py_1p5()
                    .flex()
                    .flex_col()
                    .gap_0p5();

                for (idx, item) in items.into_iter().enumerate() {
                    let settled = item.status == "completed" || item.status == "cancelled";
                    let icon_el = todo_status_icon(
                        SharedString::from(format!("todo-inline-spin-{}-{idx}", tool.tool_call_id.as_str())),
                        item.status,
                        animate_motion,
                        t,
                    );
                    let tip_title = SharedString::from(item.title.clone());
                    let tip_desc = item.description.clone().map(SharedString::from);
                    let tip_ev = item.evidence.clone().map(SharedString::from);

                    list = list.child(
                        div()
                            .id(SharedString::from(format!(
                                "todo-inline-row-{}-{idx}",
                                tool.tool_call_id.as_str()
                            )))
                            .px_1p5()
                            .py_1()
                            .rounded(px(RADIUS_CHIP))
                            .hover(|s| s.bg(t.hover))
                            .flex()
                            .items_start()
                            .gap_2()
                            .tooltip(move |_window, cx| {
                                let title = tip_title.clone();
                                let description = tip_desc.clone();
                                let evidence = tip_ev.clone();
                                cx.new(move |_| WrappingTooltip {
                                    title,
                                    description,
                                    evidence,
                                })
                                .into()
                            })
                            .child(div().pt(px(2.)).flex_shrink_0().child(icon_el))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(if settled { t.dim } else { t.text })
                                            .child(SharedString::from(item.title)),
                                    )
                                    .children(item.evidence.map(|ev| {
                                        div()
                                            .line_clamp(2)
                                            .text_ellipsis()
                                            .text_xs()
                                            .text_color(t.faint)
                                            .child(SharedString::from(format!("原因：{ev}")))
                                    })),
                            ),
                    );
                }

                if omitted > 0 {
                    list = list.child(
                        div()
                            .px_1p5()
                            .py_1()
                            .text_xs()
                            .text_color(t.faint)
                            .child(SharedString::from(format!(
                                "仅显示前 {MAX_TODO_CARD_ITEMS} 项，另省略 {omitted} 项"
                            ))),
                    );
                }

                card = card
                    .child(
                        div()
                            .h(px(3.))
                            .w_full()
                            .bg(t.surface_subtle)
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(ratio))
                                    .bg(if done == total { t.ok } else { t.accent }),
                            ),
                    )
                    .child(list);
            }
            card
        }
    }
}

fn tool_card(
    cx: &gpui_kit::App,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    tool: &ToolCallView,
    t: &Palette,
    tail_max: usize,
) -> Div {
    let name = tool.tool_name.as_str();
    let args = tool_args(store, tool);
    let exec = store
        .exec_sessions
        .iter()
        .find(|exec| exec.origin_call_id.as_ref() == Some(&tool.tool_call_id));
    let is_exec = exec.is_some() || name == "exec_command";
    let command = if let Some(exec) = exec {
        Some(exec.command.as_str())
    } else if is_exec {
        args.as_ref().and_then(|args| args.get("cmd")?.as_str())
    } else {
        None
    };
    let (status, status_color) = if let Some(exec) = exec {
        exec_state_label(&exec.state, t)
    } else {
        (state_label(tool.status), state_color(tool.status, t))
    };
    let fork_suffix = fork_suffix(name, args.as_ref());
    let title = if let Some(command) = command {
        let command = command.replace(['\r', '\n'], " ");
        format!("$ {command}  ·  {status}")
    } else if let Some(summary) = tool_summary(name, tool.status) {
        format!("{summary}{fork_suffix}")
    } else {
        format!("{name}  ·  {status}{fork_suffix}")
    };
    if let Some(data) = todo_card_data(store, tool) {
        let animate_motion = effective_motion(cx) != MotionLevel::Off;
        return render_inline_todo_card(store, entity, tool, name, data, animate_motion, t);
    }
    if tool_status_only(name) {
        return tool_status_line(name, title, tool.status, t);
    }
    let expanded = store.expanded_tools.contains(&tool.tool_call_id);
    let delta = tool
        .code_delta
        .as_ref()
        .map(|d| (d.lines_added as usize, d.lines_removed as usize));
    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    let shell = args.as_ref().and_then(|args| args.get("shell")?.as_str());
    let category = tool_exec_icon(name, is_exec, command, shell);
    let header = tool_toggle(
        entity,
        &tool.tool_call_id,
        expanded,
        tool_status_icon(&tool.tool_call_id, tool.status, t),
        category,
        title,
        delta,
        animate_motion,
        t,
    );
    if !expanded {
        return div().child(header);
    }

    // 展开卡:圆角卡片 + 顶部柔和头栏 + 状态 pill。
    let mut card = div()
        .flex()
        .flex_col()
        .max_w_full()
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(t.border_subtle)
        .bg(t.card)
        .shadow(shadow_card(t))
        .overflow_hidden()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .px_1p5()
                .py_1()
                .bg(t.surface_subtle)
                .border_b_1()
                .border_color(t.border_subtle)
                .child(div().flex_1().min_w_0().child(header))
                .child(qaqh_gpui_kit::kit::pill(status, status_color, t)),
        );

    let mut body = div().flex().flex_col().gap_2().p_3().min_w_0();
    if !is_exec && let Some(args) = &args {
        body = body.children(args_summary(args).map(|summary| {
            div()
                .px_2p5()
                .py_1()
                .rounded(px(RADIUS_CHIP))
                .bg(t.surface_subtle)
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_xs()
                .text_color(t.dim)
                .truncate()
                .child(SharedString::from(summary))
        }));
    }
    let raw_output: String = if let Some(exec) = exec {
        store
            .exec_output_tail
            .iter()
            .find(|(id, end, text)| {
                id == &exec.exec_session_id && *end > exec.next_offset && !text.is_empty()
            })
            .map_or_else(
                || exec.output_preview.clone(),
                |(_, _, text)| text.clone(),
            )
    } else if is_terminal_status(tool.status) {
        tool.output
            .as_ref()
            .map(|content| store.content_text(content))
            .unwrap_or_default()
    } else {
        store
            .output_tail
            .iter()
            .find(|(call_id, _)| call_id == &tool.tool_call_id)
            .map(|(_, tail)| tail.clone())
            .unwrap_or_default()
    };
    let result = if is_exec && exec.is_none() {
        serde_json::from_str::<serde_json::Value>(&raw_output).ok()
    } else {
        None
    };
    let output = result
        .as_ref()
        .and_then(|result| result.get("output")?.as_str())
        .map(str::to_owned)
        .unwrap_or(raw_output);
    let (output, truncated) = utf8_tail(&output, tail_max);
    if !output.is_empty() {
        body = body.child(
            div()
                .id(SharedString::from(format!("tool-output-{}", tool.tool_call_id.as_str())))
                .max_h(px(240.))
                .overflow_y_scroll()
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_xs()
                .px_3()
                .py_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(t.code_bg)
                .text_color(t.dim)
                .min_w_0()
                .child(SharedString::from(if truncated {
                    format!("…{output}")
                } else {
                    output.to_string()
                })),
        );
        if truncated {
            body = body.child(
                div()
                    .text_xs()
                    .text_color(t.faint)
                    .child("(仅显示输出尾部)"),
            );
        }
    }
    if let Some(exec) = exec
        && !exec.finished
    {
        let id = exec.exec_session_id.as_str().to_owned();
        let control = |terminate| {
            let entity = entity.downgrade();
            let exec_id = exec.exec_session_id.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut gpui_kit::App| {
                let _ = entity.update(cx, |store, _| store.exec_control(exec_id.clone(), terminate));
            }
        };
        body = body.child(
            div()
                .flex()
                .gap_2()
                .child(action_button(
                    cx,
                    format!("exec-int-{id}"),
                    "中断",
                    t.warn,
                    control(false),
                    t,
                ))
                .child(action_button(
                    cx,
                    format!("exec-term-{id}"),
                    "终止",
                    t.bad,
                    control(true),
                    t,
                )),
        );
    }
    if let Some(exec) = exec {
        let exec_id = exec.exec_session_id.as_str().to_owned();
        let exec_handle = exec.exec_session_id.clone();
        let entity = entity.downgrade();
        body = body.child(
            div()
                .id(SharedString::from(format!(
                    "exec-open-terminal-{}",
                    exec_id
                )))
                .self_start()
                .px_2p5()
                .py_1()
                .rounded(px(RADIUS_CHIP))
                .bg(t.accent_soft)
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .text_color(t.accent)
                .hover(|style| style.bg(t.accent_bg))
                .child("在执行终端面板中查看 →")
                .on_click(move |_, _, cx| {
                    let _ = entity.update(cx, |store, cx| {
                        crate::terminal_panel::open(store, Some(exec_handle.clone()), cx);
                    });
                }),
        );
    }
    card = card.child(body);
    card
}

/// 连续成功工具调用「无框轨迹脊柱」:Antigravity 极简紧凑行 + 1px 左侧脊柱 + MotionReveal 弹簧展开。
fn tool_burst_card(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    entity: &Entity<SessionStore>,
    group_id: usize,
    indices: &[usize],
    t: &Palette,
    tail_max: usize,
) -> Div {
    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    let (expanded, summary) = {
        let store = entity.read(cx);
        (
            store.expanded_groups.contains(&group_id),
            summarize_burst(store, indices),
        )
    };
    let weak = entity.downgrade();
    let header = div()
        .id(SharedString::from(format!("tool-burst-{group_id}")))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .cursor_pointer()
        .hover(|style| style.bg(t.hover))
        .on_click(move |_, _, cx| {
            let _ = weak.update(cx, |store, cx| {
                if !store.expanded_groups.remove(&group_id) {
                    store.expanded_groups.insert(group_id);
                }
                cx.notify();
            });
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .flex_1()
                .child(
                    Icon::new(IconName::CircleCheck)
                        .with_size(Size::XSmall)
                        .text_color(t.ok),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.dim)
                        .truncate()
                        .child(SharedString::from(summary)),
                ),
        )
        .child(spring_chevron(
            SharedString::from(format!("burst-chev-{group_id}")),
            expanded,
            t.faint,
            animate_motion,
        ));

    let entity_for_spine = entity.clone();
    let indices_owned = indices.to_vec();
    let t_copy = *t;
    let revealed = accordion_reveal(
        SharedString::from(format!("burst-reveal-{group_id}")),
        expanded,
        window,
        cx,
        move |cx| {
            let store = entity_for_spine.read(cx);
            let mut spine = div()
                .mt_1()
                .ml_2p5()
                .pl_3()
                .border_l_1()
                .border_color(t_copy.border_subtle)
                .flex()
                .flex_col()
                .gap_1();
            for &idx in &indices_owned {
                if let Some(call_id) = store
                    .entries
                    .get(idx)
                    .filter(|block| block.kind == BlockKind::ToolCall)
                    .and_then(|block| block.tool_call_id.as_ref())
                    && let Some(tool) = store.tools.iter().find(|tool| &tool.tool_call_id == call_id)
                {
                    spine = spine.child(tool_card(
                        cx,
                        store,
                        &entity_for_spine,
                        tool,
                        &t_copy,
                        tail_max,
                    ));
                }
            }
            spine.into_any_element()
        },
    );

    div()
        .flex()
        .flex_col()
        .child(header)
        .children(revealed)
}

fn format_elapsed_cn(ms: u64, live: bool) -> Option<String> {
    if ms == 0 && !live {
        return None;
    }
    if ms < 1_000 {
        return Some(if live {
            "0秒".to_owned()
        } else {
            format!("{ms}ms")
        });
    }
    let secs = ms / 1_000;
    if secs < 60 {
        Some(format!("{secs}秒"))
    } else if secs < 3_600 {
        Some(format!("{}分{}秒", secs / 60, secs % 60))
    } else {
        Some(format!("{}小时{}分", secs / 3_600, (secs % 3_600) / 60))
    }
}

fn work_trace_elapsed_ms(
    store: &SessionStore,
    trace_key: &str,
    indices: &[usize],
    is_live: bool,
) -> Option<u64> {
    if is_live {
        return store
            .trace_started_at
            .get(trace_key)
            .map(|started| started.elapsed().as_millis() as u64);
    }
    if let Some(&ms) = store.trace_elapsed_ms.get(trace_key) {
        return Some(ms);
    }
    let mut min_start: Option<i64> = None;
    let mut max_end: Option<i64> = None;
    let mut gen_ms: u64 = 0;
    for &idx in indices {
        match store.entries.get(idx) {
            Some(block) if block.kind == BlockKind::ToolCall => {
                if let Some(tool) = block.tool_call_id.as_ref().and_then(|id| find_tool(store, id))
                    && let Some(metrics) = &tool.metrics
                {
                    let started = metrics.started_at_ms as i64;
                    let finished = if metrics.finished_at_ms == 0 {
                        started
                    } else {
                        metrics.finished_at_ms as i64
                    };
                    min_start = Some(min_start.map_or(started, |v| v.min(started)));
                    max_end = Some(max_end.map_or(finished, |v| v.max(finished)));
                }
            }
            Some(block) if block.generated_ms.is_some() => {
                gen_ms = gen_ms.saturating_add(block.generated_ms.unwrap_or_default());
            }
            _ => {}
        }
    }
    if let (Some(s), Some(e)) = (min_start, max_end)
        && e >= s
    {
        return Some((e - s).max(0) as u64);
    }
    (gen_ms > 0).then_some(gen_ms)
}

fn classify_tool_bucket(store: &SessionStore, tool: &ToolCallView) -> &'static str {
    let name = tool.tool_name.as_str();
    let is_exec = name == "exec_command"
        || store
            .exec_sessions
            .iter()
            .any(|e| e.origin_call_id.as_ref() == Some(&tool.tool_call_id));
    if is_exec {
        "cmd"
    } else if matches!(name, "edit" | "apply_patch" | "write" | "delete") {
        "file"
    } else if matches!(name, "read" | "read_image" | "glob" | "grep" | "search" | "web_fetch") {
        "search"
    } else if matches!(name, "todo_write" | "todo_update" | "todo_list") {
        "todo"
    } else if name.starts_with("spawn_")
        || name.ends_with("_agent")
        || name.starts_with("task_")
        || name.starts_with("board_")
    {
        "agent"
    } else {
        "other"
    }
}

fn summarize_work_trace(store: &SessionStore, indices: &[usize], attach_live: bool) -> String {
    let (mut cmds, mut files, mut searches, mut todos, mut agents, mut others) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let (mut reasonings, mut retracted_answers) = (0usize, 0usize);

    let mut bump_tool = |bucket: &str| match bucket {
        "cmd" => cmds += 1,
        "file" => files += 1,
        "search" => searches += 1,
        "todo" => todos += 1,
        "agent" => agents += 1,
        _ => others += 1,
    };

    for &idx in indices {
        match store.entries.get(idx).map(|block| block.kind) {
            Some(BlockKind::ToolCall) => {
                let tool = store
                    .entries
                    .get(idx)
                    .and_then(|block| block.tool_call_id.as_ref())
                    .and_then(|call_id| find_tool(store, call_id));
                match tool {
                    Some(tool) => bump_tool(classify_tool_bucket(store, tool)),
                    None => bump_tool("other"),
                }
            }
            Some(BlockKind::Reasoning) => reasonings += 1,
            Some(BlockKind::Answer) => retracted_answers += 1,
            _ => {}
        }
    }

    if attach_live {
        let referenced: HashSet<&ToolCallId> =
            store.entries.iter().filter_map(|block| block.tool_call_id.as_ref()).collect();
        for (_, kind, text) in &store.streaming {
            if *kind == BlockKind::Reasoning && has_assistant_text(*kind, text) {
                reasonings += 1;
            }
        }
        for tool in &store.tools {
            if !referenced.contains(&tool.tool_call_id) && is_trace_foldable_tool_view(store, tool) {
                bump_tool(classify_tool_bucket(store, tool));
            }
        }
        drop(bump_tool);
        others += store.arg_stream.len();
    }

    let mut parts = Vec::new();
    if searches > 0 {
        parts.push(format!("查阅 ×{searches}"));
    }
    if files > 0 {
        parts.push(format!("改写 ×{files}"));
    }
    if cmds > 0 {
        parts.push(format!("命令 ×{cmds}"));
    }
    if todos > 0 {
        parts.push(format!("待办 ×{todos}"));
    }
    if agents > 0 {
        parts.push(format!("协作 ×{agents}"));
    }
    if others > 0 {
        parts.push(format!("工具 ×{others}"));
    }
    if reasonings > 0 {
        parts.push(format!("思考 ×{reasonings}"));
    }
    if retracted_answers > 0 {
        parts.push(format!("中间消息 ×{retracted_answers}"));
    }

    if attach_live {
        if parts.is_empty() {
            String::new()
        } else {
            parts.join(" · ")
        }
    } else if parts.is_empty() {
        format!("已完成 {} 步工作过程", indices.len())
    } else {
        format!("已完成工作过程 · {}", parts.join(" · "))
    }
}

fn render_arg_stream_item(
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    stream: &ArgStream,
    animate_motion: bool,
    t: &Palette,
) -> AnyElement {
    let ArgStream {
        call_id,
        tool_name,
        text,
        lines,
        target_files,
        args_complete,
    } = stream;
    if tool_status_only(tool_name) {
        let title = tool_summary(tool_name, ToolCallStatus::Pending)
            .map_or_else(|| format!("正在准备 {tool_name}"), str::to_owned);
        return tool_status_line(tool_name, title, ToolCallStatus::Pending, t).into_any_element();
    }
    let mut title = format!("正在准备 {tool_name}");
    if let Some((added, removed)) = lines {
        title.push_str(&format!("  +{added} −{removed}"));
    }
    if !target_files.is_empty() {
        title.push_str(&format!("  · {}", target_files.join(", ")));
    }
    if *args_complete {
        title.push_str("  · 参数已就绪");
    }
    let parsed_args = (*args_complete)
        .then(|| serde_json::from_str::<serde_json::Value>(text).ok())
        .flatten();
    let stream_cmd = parsed_args
        .as_ref()
        .and_then(|args| args.get("cmd")?.as_str());
    let stream_shell = parsed_args
        .as_ref()
        .and_then(|args| args.get("shell")?.as_str());
    let expanded = store.expanded_tools.contains(call_id);
    div()
        .child(tool_toggle(
            entity,
            call_id,
            expanded,
            tool_status_icon(call_id, ToolCallStatus::Running, t),
            tool_exec_icon(
                tool_name,
                tool_name == "exec_command",
                stream_cmd,
                stream_shell,
            ),
            title,
            None,
            animate_motion,
            t,
        ))
        .children(expanded.then(|| {
            div()
                .px_3()
                .py_1p5()
                .rounded(px(RADIUS_CHIP))
                .bg(t.code_bg)
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_xs()
                .text_color(t.dim)
                .max_h(px(240.))
                .id(SharedString::from(format!("arg-stream-{}", call_id.as_str())))
                .overflow_y_scroll()
                .child(SharedString::from(text.clone()))
        }))
        .into_any_element()
}

/// 已退回折叠区的中间正文行:紧凑预览 + 点击手风琴展开完整正文。
#[allow(clippy::too_many_arguments)]
fn retracted_answer_row(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    entity: &Entity<SessionStore>,
    source: AssistantSource,
    block_id: &BlockId,
    section: usize,
    t: &Palette,
    ty: &settings::Typography,
) -> AnyElement {
    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    let weak = entity.downgrade();
    let (expanded, preview, md_key, md_extensions) = {
        let store = entity.read(cx);
        let text = assistant_source_text(store, source);
        let first = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("");
        let snippet: String = first.chars().take(52).collect();
        let preview = if snippet.is_empty() {
            "中间消息".to_owned()
        } else {
            format!(
                "中间消息 · {snippet}{}",
                if first.chars().count() > 52 { "…" } else { "" }
            )
        };
        (
            store.expanded_thinking.contains(block_id),
            preview,
            SharedString::from(format!("md-retracted-{}-{}-{section}", store.session_id, block_id.as_str())),
            store.md_extensions.clone(),
        )
    };
    let toggle_block = block_id.clone();
    let header = div()
        .id(SharedString::from(format!("retracted-toggle-{}", block_id.as_str())))
        .flex()
        .items_center()
        .gap_2()
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .text_xs()
        .text_color(t.dim)
        .cursor_pointer()
        .hover(|style| style.bg(t.hover).text_color(t.text))
        .child(
            Icon::new(IconName::MessageSquare)
                .with_size(Size::XSmall)
                .text_color(t.dim),
        )
        .child(div().flex_1().min_w_0().truncate().child(SharedString::from(preview)))
        .child(spring_chevron(
            SharedString::from(format!("retracted-chev-{}", block_id.as_str())),
            expanded,
            t.faint,
            animate_motion,
        ))
        .on_click(move |_, _, cx| {
            let _ = weak.update(cx, |store, cx| {
                if !store.expanded_thinking.remove(&toggle_block) {
                    store.expanded_thinking.insert(toggle_block.clone());
                }
                cx.notify();
            });
        });

    let entity_for_body = entity.clone();
    let t_copy = *t;
    let ty_copy = *ty;
    let revealed = accordion_reveal(
        SharedString::from(format!("retracted-reveal-{}", block_id.as_str())),
        expanded,
        window,
        cx,
        move |cx| {
            let store = entity_for_body.read(cx);
            let text = assistant_source_text(store, source);
            let body = crate::markdown::render_document(
                md_key,
                &text,
                &md_extensions,
                entity_for_body.downgrade(),
                &t_copy,
                &ty_copy,
            );
            div()
                .ml_2p5()
                .pl_3()
                .border_l_1()
                .border_color(t_copy.border_subtle)
                .child(body)
                .into_any_element()
        },
    );

    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(header)
        .children(revealed)
        .into_any_element()
}

/// 折叠模式下的「工作过程有限视窗」:
/// - 进行中默认展开为固定上限高度(224px)内部可滚动视窗 + 上下渐隐遮罩 + 实时计时;
/// - 收口后默认折叠为单行摘要 + 耗时,随时可手风琴展开回看历史步骤与旧中间回复。
#[allow(clippy::too_many_arguments)]
fn work_trace_card(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    entity: &Entity<SessionStore>,
    trace_key: String,
    indices: &[usize],
    attach_live_tail: bool,
    t: &Palette,
    ty: &settings::Typography,
    tail_max: usize,
) -> Div {
    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    let (expanded, summary, elapsed_label, scroll_handle) = {
        let store = entity.read(cx);
        let expanded = if attach_live_tail {
            !store.collapsed_traces.contains(&trace_key)
        } else {
            store.expanded_traces.contains(&trace_key)
        };
        let summary = summarize_work_trace(store, indices, attach_live_tail);
        let elapsed_label = work_trace_elapsed_ms(store, &trace_key, indices, attach_live_tail)
            .and_then(|ms| format_elapsed_cn(ms, attach_live_tail));
        let scroll_handle = store
            .trace_scroll_handles
            .get(&trace_key)
            .cloned()
            .unwrap_or_else(ScrollHandle::new);
        (expanded, summary, elapsed_label, scroll_handle)
    };

    let weak = entity.downgrade();
    let toggle_key = trace_key.clone();
    let header_title: AnyElement = if attach_live_tail {
        let mut row = div()
            .flex()
            .items_center()
            .gap_1p5()
            .min_w_0()
            .flex_1()
            .child(shimmer_label(
                SharedString::from(format!("trace-shimmer-{trace_key}")),
                "正在执行中",
                t,
            ));
        if let Some(dur) = &elapsed_label {
            row = row
                .child(div().text_xs().text_color(t.faint).child("·"))
                .child(
                    div()
                        .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                        .text_xs()
                        .text_color(t.dim)
                        .child(SharedString::from(dur.clone())),
                );
        }
        if !summary.is_empty() {
            row = row
                .child(div().text_xs().text_color(t.faint).child("·"))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.dim)
                        .truncate()
                        .child(SharedString::from(summary)),
                );
        }
        row.into_any_element()
    } else {
        let mut row = div()
            .flex()
            .items_center()
            .gap_1p5()
            .min_w_0()
            .flex_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.dim)
                    .truncate()
                    .child(SharedString::from(summary)),
            );
        if let Some(dur) = &elapsed_label {
            row = row
                .child(div().text_xs().text_color(t.faint).child("·"))
                .child(
                    div()
                        .flex_shrink_0()
                        .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                        .text_xs()
                        .text_color(t.faint)
                        .child(SharedString::from(dur.clone())),
                );
        }
        row.into_any_element()
    };

    let header = div()
        .id(SharedString::from(format!("work-trace-{trace_key}")))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .cursor_pointer()
        .hover(|style| style.bg(t.hover))
        .on_click(move |_, _, cx| {
            let _ = weak.update(cx, |store, cx| {
                if attach_live_tail {
                    if !store.collapsed_traces.remove(&toggle_key) {
                        store.collapsed_traces.insert(toggle_key.clone());
                    }
                } else if !store.expanded_traces.remove(&toggle_key) {
                    store.expanded_traces.insert(toggle_key.clone());
                }
                cx.notify();
            });
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .flex_1()
                .child(if attach_live_tail {
                    live_dot(format!("trace-dot-{trace_key}"), t.accent, true)
                } else {
                    Icon::new(IconName::CircleCheck)
                        .with_size(Size::XSmall)
                        .text_color(t.ok)
                        .into_any_element()
                })
                .child(header_title),
        )
        .child(spring_chevron(
            SharedString::from(format!("trace-chev-{trace_key}")),
            expanded,
            t.faint,
            animate_motion,
        ));

    let entity_for_body = entity.clone();
    let indices_owned = indices.to_vec();
    let t_copy = *t;
    let ty_copy = *ty;
    let key_for_body = trace_key.clone();

    let mut body_rows = Vec::new();
    if expanded {
        enum TraceSlot {
            EntryTool(ToolCallId),
            Assistant {
                kind: BlockKind,
                source: AssistantSource,
                live: bool,
                section: usize,
                block_id: BlockId,
            },
            ArgStream(usize),
            UnrefTool(ToolCallId),
        }

        let slots: Vec<TraceSlot> = {
            let store = entity_for_body.read(cx);
            let mut slots = Vec::new();
            for &idx in &indices_owned {
                match store.entries.get(idx) {
                    Some(block) if block.kind == BlockKind::ToolCall => {
                        if let Some(call_id) = block.tool_call_id.as_ref() {
                            slots.push(TraceSlot::EntryTool(call_id.clone()));
                        }
                    }
                    Some(block) if matches!(block.kind, BlockKind::Answer | BlockKind::Reasoning) => {
                        slots.push(TraceSlot::Assistant {
                            kind: block.kind,
                            source: AssistantSource::Entry(idx),
                            live: false,
                            section: idx * 2,
                            block_id: block.block_id.clone(),
                        });
                    }
                    _ => {}
                }
            }
            if attach_live_tail {
                for (i, (block_id, kind, text)) in store.streaming.iter().enumerate() {
                    if *kind != BlockKind::Answer && has_assistant_text(*kind, text) {
                        slots.push(TraceSlot::Assistant {
                            kind: *kind,
                            source: AssistantSource::Streaming(i),
                            live: true,
                            section: i * 2 + 1,
                            block_id: block_id.clone(),
                        });
                    }
                }
                for i in 0..store.arg_stream.len() {
                    slots.push(TraceSlot::ArgStream(i));
                }
                let referenced: HashSet<&ToolCallId> =
                    store.entries.iter().filter_map(|block| block.tool_call_id.as_ref()).collect();
                for tool in &store.tools {
                    if !referenced.contains(&tool.tool_call_id) && is_trace_foldable_tool_view(store, tool) {
                        slots.push(TraceSlot::UnrefTool(tool.tool_call_id.clone()));
                    }
                }
            }
            slots
        };

        for slot in slots {
            match slot {
                TraceSlot::EntryTool(call_id) | TraceSlot::UnrefTool(call_id) => {
                    let store = entity_for_body.read(cx);
                    if let Some(tool) = find_tool(store, &call_id) {
                        body_rows.push(
                            tool_card(cx, store, &entity_for_body, tool, &t_copy, tail_max)
                                .into_any_element(),
                        );
                    }
                }
                TraceSlot::Assistant {
                    kind: BlockKind::Answer,
                    source,
                    section,
                    block_id,
                    ..
                } => {
                    body_rows.push(retracted_answer_row(
                        window,
                        cx,
                        &entity_for_body,
                        source,
                        &block_id,
                        section,
                        &t_copy,
                        &ty_copy,
                    ));
                }
                TraceSlot::Assistant {
                    kind,
                    source,
                    live,
                    section,
                    block_id,
                } => {
                    body_rows.push(assistant_section(
                        window,
                        cx,
                        &kind,
                        source,
                        live,
                        section,
                        &block_id,
                        &entity_for_body,
                        &t_copy,
                        &ty_copy,
                    ));
                }
                TraceSlot::ArgStream(i) => {
                    let store = entity_for_body.read(cx);
                    if let Some(stream) = store.arg_stream.get(i) {
                        body_rows.push(render_arg_stream_item(
                            store,
                            &entity_for_body,
                            stream,
                            animate_motion,
                            &t_copy,
                        ));
                    }
                }
            }
        }
    }

    let revealed = accordion_reveal(
        SharedString::from(format!("trace-reveal-{trace_key}")),
        expanded,
        window,
        cx,
        move |_cx| {
            let scroll_list = div()
                .id(SharedString::from(format!("trace-scroll-{key_for_body}")))
                .max_h(px(224.))
                .w_full()
                .overflow_y_scroll()
                .track_scroll(&scroll_handle)
                .py_1p5()
                .pr_2p5()
                .flex()
                .flex_col()
                .gap_1()
                .children(body_rows);

            div()
                .relative()
                .mt_1()
                .ml_2p5()
                .pl_3()
                .border_l_1()
                .border_color(t_copy.border_subtle)
                .vertical_scrollbar(&scroll_handle)
                .child(scroll_list)
                .child(
                    top_fade_mask(t_copy.stage, 12.)
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0(),
                )
                .child(
                    bottom_fade_mask(t_copy.stage, 12.)
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .right_0(),
                )
                .into_any_element()
        },
    );

    div()
        .flex()
        .flex_col()
        .child(header)
        .children(revealed)
}

/// Provider 重试横幅:居中圆角警示胶囊。
pub(crate) fn provider_retry_banner(store: &SessionStore, t: &Palette) -> Option<Div> {
    let retry = store.provider_retry.as_ref()?;
    Some(
        div()
            .mx_4()
            .mb_1p5()
            .px_3()
            .py_1p5()
            .rounded(px(RADIUS_CONTROL))
            .bg(soft(t.warn, 0.12))
            .border_1()
            .border_color(soft(t.warn, 0.25))
            .flex()
            .items_center()
            .gap_2()
            .child(live_dot("provider-retry", t.warn, true))
            .child(
                div()
                    .text_xs()
                    .text_color(t.warn)
                    .child(SharedString::from(format!(
                        "模型请求重试中 ({}/{}) · {}s 后重试 · {}",
                        retry.attempt, retry.max_attempts, retry.delay_secs, retry.error
                    ))),
            ),
    )
}

fn is_terminal_status(status: ToolCallStatus) -> bool {
    matches!(
        status,
        ToolCallStatus::Succeeded
            | ToolCallStatus::Failed
            | ToolCallStatus::Cancelled
            | ToolCallStatus::Indeterminate
    )
}

fn has_assistant_text(kind: BlockKind, text: &str) -> bool {
    matches!(kind, BlockKind::Answer | BlockKind::Reasoning) && !text.trim().is_empty()
}

fn hidden_timeline_entry(store: &SessionStore, block: &BlockView) -> bool {
    match block.kind {
        BlockKind::ToolResult => true,
        BlockKind::Answer | BlockKind::Reasoning => {
            !has_assistant_text(block.kind, &content_text(store, &block.content))
        }
        BlockKind::System => content_text(store, &block.content).trim().is_empty(),
        BlockKind::User | BlockKind::ToolCall => false,
    }
}

/// 思考链路折叠头:Antigravity 无框行 + Sparkles 图标 + 流式中性扫光文字 + 物理弹簧箭头。
fn thinking_toggle(
    entity: &Entity<SessionStore>,
    block_id: &BlockId,
    expanded: bool,
    live: bool,
    title: String,
    animate_motion: bool,
    t: &Palette,
) -> gpui_kit::Stateful<Div> {
    let entity = entity.downgrade();
    let block_id = block_id.clone();
    let label_el: AnyElement = if live && !expanded {
        shimmer_label(
            format!("thinking-shimmer-{}", block_id.as_str()),
            "正在深度思考…",
            t,
        )
    } else {
        div()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.dim)
            .truncate()
            .child(SharedString::from(title))
            .into_any_element()
    };

    div()
        .id(SharedString::from(format!("thinking-toggle-{}", block_id.as_str())))
        .self_start()
        .max_w_full()
        .flex()
        .items_center()
        .gap_1p5()
        .px_1p5()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .cursor_pointer()
        .hover(|style| style.bg(t.hover))
        .child(
            Icon::new(IconName::Sparkles)
                .with_size(Size::XSmall)
                .text_color(if live { t.text } else { t.dim }),
        )
        .child(div().min_w_0().flex_1().child(label_el))
        .child(spring_chevron(
            SharedString::from(format!("think-chev-{}", block_id.as_str())),
            expanded,
            t.faint,
            animate_motion,
        ))
        .on_click(move |_, _, cx| {
            let _ = entity.update(cx, |store, cx| {
                if !store.expanded_thinking.remove(&block_id) {
                    store.expanded_thinking.insert(block_id.clone());
                }
                cx.notify();
            });
        })
}

#[derive(Clone, Copy)]
enum AssistantSource {
    Entry(usize),
    Streaming(usize),
}

fn assistant_source_text(store: &SessionStore, source: AssistantSource) -> String {
    match source {
        AssistantSource::Entry(idx) => match store.entries.get(idx) {
            Some(block) => content_text(store, &block.content),
            None => String::new(),
        },
        AssistantSource::Streaming(idx) => store
            .streaming
            .get(idx)
            .map(|(_, _, text)| text.clone())
            .unwrap_or_default(),
    }
}

/// 标准正文阅读轨(最宽 880px,窄屏或分栏压缩时自动收缩),表格破栏块以外的所有元素统一走此轨道居中。
fn prose_rail(_ty: &settings::Typography) -> Div {
    div()
        .w_full()
        .max_w(px(MESSAGE_MAX_WIDTH_PX))
        .mx_auto()
        .px_4()
}

/// 回到最新消息悬浮图标按钮:仅在偏离底部时经 `presence_overlay` 居中显现,纯 ↓ 图标无文字。
fn scroll_to_latest_button(
    store: &SessionStore,
    t: &Palette,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
) -> Option<AnyElement> {
    let list = &store.timeline_list;
    let remaining = (f32::from(list.max_offset_for_scrollbar().y)
        + f32::from(list.scroll_px_offset_for_scrollbar().y))
    .max(0.0);
    let away_from_bottom = !list.is_following_tail()
        && (list.is_scrolled_to_end() == Some(false) || remaining > 40.0)
        && remaining > 24.0;
    let sample = presence_overlay("timeline-scroll-latest", away_from_bottom, 160, window, cx);
    if !sample.should_render() {
        return None;
    }
    let p = sample.progress.clamp(0.0, 1.0);
    let shift = if effective_motion(cx) == MotionLevel::Full {
        (1.0 - p) * 8.0
    } else {
        0.0
    };
    Some(
        div()
            .absolute()
            .bottom_3()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("scroll-to-latest-msg")
                    .size(px(32.))
                    .mt(px(shift))
                    .opacity(p)
                    .rounded_full()
                    .bg(t.card)
                    .border_1()
                    .border_color(t.border)
                    .shadow(shadow_card(t))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|style| style.bg(t.card_hover))
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new("回到最新消息")
                            .build(window, cx)
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|store, _, _, cx| {
                        store.timeline_list.scroll_to_end();
                        if settings::get(cx).behavior.stream_follow {
                            store.timeline_list.set_follow_mode(FollowMode::Tail);
                        }
                        cx.notify();
                    }))
                    .child(
                        Icon::new(IconName::ArrowDown)
                            .with_size(Size::XSmall)
                            .text_color(t.text),
                    ),
            )
            .into_any_element(),
    )
}

/// 助手块 markdown 区(Reasoning 使用无框思考行 + 左侧 1px 脊柱 MotionReveal 展开)。
#[allow(clippy::too_many_arguments)]
fn assistant_section(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    kind: &BlockKind,
    source: AssistantSource,
    live: bool,
    section: usize,
    block_id: &BlockId,
    entity: &Entity<SessionStore>,
    t: &Palette,
    ty: &settings::Typography,
) -> AnyElement {
    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    let reasoning = matches!(kind, BlockKind::Reasoning);
    let weak_entity = entity.downgrade();
    let (expanded, header, body, full_text, md_extensions, md_key) = {
        let store = entity.read(cx);
        let text = assistant_source_text(store, source);
        let md_key = SharedString::from(format!(
            "md-{}-{}-{section}",
            store.session_id, block_id.as_str()
        ));
        let md_extensions = store.md_extensions.clone();
        let expanded = reasoning && store.expanded_thinking.contains(block_id);
        let collapsed = reasoning && !expanded;
        let header: Option<AnyElement> = if reasoning {
            let mut title = "思考过程".to_owned();
            if collapsed {
                let first = text
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("");
                let preview: String = first.chars().take(48).collect();
                if !preview.is_empty() {
                    title.push_str(&format!(
                        " · {preview}{}",
                        if first.chars().count() > 48 { "…" } else { "" }
                    ));
                }
            }
            Some(
                thinking_toggle(entity, block_id, expanded, live, title, animate_motion, t)
                    .into_any_element(),
            )
        } else if matches!(kind, BlockKind::ToolCall) {
            Some(
                prose_rail(ty)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.faint)
                            .child("工具调用"),
                    )
                    .into_any_element(),
            )
        } else {
            None
        };
        let body = if reasoning {
            None
        } else {
            Some(crate::markdown::render_document_breakout(
                md_key.clone(),
                &text,
                &md_extensions,
                weak_entity.clone(),
                t,
                ty,
            ))
        };
        (expanded, header, body, text.to_owned(), md_extensions, md_key)
    };

    let mut container = div()
        .id(("mdsec", section))
        .w_full()
        .flex()
        .flex_col()
        .gap_1p5()
        .max_w_full()
        .children(header);

    if reasoning {
        let entity_for_body = entity.clone();
        let t_copy = *t;
        let ty_copy = *ty;
        let md_ext_for_body = md_extensions.clone();
        let md_key_for_body = md_key.clone();
        let revealed = accordion_reveal(
            SharedString::from(format!("think-reveal-{}", block_id.as_str())),
            expanded,
            window,
            cx,
            move |cx| {
                let store = entity_for_body.read(cx);
                let text = assistant_source_text(store, source);
                let body = crate::markdown::render_document(
                    md_key_for_body.clone(),
                    &text,
                    &md_ext_for_body,
                    entity_for_body.downgrade(),
                    &t_copy,
                    &ty_copy,
                );
                div()
                    .ml_2p5()
                    .pl_3()
                    .border_l_1()
                    .border_color(t_copy.border_subtle)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(body)
                    .into_any_element()
            },
        );
        container = container.children(revealed);
    } else if let Some(body) = body {
        container = container.child(body);
    }

    let section_el = container
        .children((live && !reasoning).then(|| {
            prose_rail(ty).child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .pt_0p5()
                    .child(live_dot(format!("live-gen-{}", block_id.as_str()), t.accent, true))
                    .child(shimmer_label(
                        format!("live-shimmer-{}", block_id.as_str()),
                        "正在生成…",
                        t,
                    )),
            )
        }))
        .context_menu(move |menu, window, cx| {
            copy_context_menu(menu, window, cx, &full_text, &weak_entity)
        });

    if reasoning {
        prose_rail(ty).child(section_el).into_any_element()
    } else {
        section_el.into_any_element()
    }
}

/// 通知行:左侧呼吸点/色点 + 标签 + 正文。
fn notice_line(label: &str, text: &str, color: Hsla, t: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3p5()
        .py_2()
        .rounded(px(RADIUS_CARD))
        .bg(t.surface_subtle)
        .border_1()
        .border_color(t.border_subtle)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(div().size_1p5().rounded_full().bg(color))
                .child(SharedString::from(label.to_owned())),
        )
        .child(
            div()
                .text_sm()
                .text_color(t.text)
                .child(SharedString::from(text.to_owned())),
        )
}

fn terminal_notice(
    terminal: &qaqh_ringing_v3::SubagentTerminalView,
    text: &str,
    t: &Palette,
) -> Div {
    use qaqh_ringing_v3::SubagentTerminalKind;
    let (label, color) = match terminal.terminal {
        SubagentTerminalKind::Completed => ("子代理已完成", t.ok),
        SubagentTerminalKind::Failed => ("子代理失败", t.bad),
        SubagentTerminalKind::Cancelled => ("子代理已取消", t.dim),
        SubagentTerminalKind::TimedOut => ("子代理超时", t.warn),
    };
    let title = format!(
        "{label} · {} (调用 {})",
        terminal.child_session_id.as_str(),
        terminal.parent_call_id.as_str()
    );
    notice_line(&title, text, color, t)
}

#[allow(clippy::too_many_arguments)]
fn timeline_entry(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    entity: &Entity<SessionStore>,
    index: usize,
    t: &Palette,
    ty: &settings::Typography,
    tail_max: usize,
) -> Option<AnyElement> {
    let assistant_meta = {
        let store = entity.read(cx);
        match store.entries.get(index)? {
            BlockView {
                block_id,
                kind: block_kind @ (BlockKind::Answer | BlockKind::Reasoning),
                ..
            } => Some((block_id.clone(), *block_kind)),
            _ => None,
        }
    };
    if let Some((block_id, block_kind)) = assistant_meta {
        return Some(
            assistant_section(
                window,
                cx,
                &block_kind,
                AssistantSource::Entry(index),
                false,
                index * 2,
                &block_id,
                entity,
                t,
                ty,
            )
            .into_any_element(),
        );
    }

    let open_info = {
        let store = entity.read(cx);
        if store
            .entries
            .get(index)
            .is_some_and(|block| block.kind == BlockKind::User)
        {
            crate::handoff::detect_open_source(store, index).map(|src| {
                let text = match store.entries.get(index) {
                    Some(block) => content_text(store, &block.content),
                    _ => String::new(),
                };
                (src, text)
            })
        } else {
            None
        }
    };
    if let Some((open_source, open_text)) = open_info {
        return Some(
            crate::handoff::render_open_card(
                window,
                cx,
                entity,
                &open_text,
                &open_source,
                t,
                ty,
            )
            .into_any_element(),
        );
    }

    let seal_facts = {
        let store = entity.read(cx);
        if let Some(call_id) = store
            .entries
            .get(index)
            .filter(|block| block.kind == BlockKind::ToolCall)
            .and_then(|block| block.tool_call_id.as_ref())
        {
            store
                .tools
                .iter()
                .find(|tool| &tool.tool_call_id == call_id)
                .and_then(|tool| {
                    crate::store::handoff_facts(tool, |content| store.content_text(content))
                })
        } else {
            None
        }
    };
    if let Some(facts) = seal_facts {
        return Some(
            crate::handoff::render_seal_card(window, cx, entity, &facts, t, ty)
                .into_any_element(),
        );
    }

    let store = entity.read(cx);
    let entry = store.entries.get(index)?;
    Some(match entry.kind {
        BlockKind::User => {
            let text = content_text(store, &entry.content);
            let actor = entry.actor.unwrap_or(BlockActor::User);
            let purpose = entry.purpose.unwrap_or(InputPurpose::TriggerTurn);
            let turn_id = entry.turn_id.as_ref();
            let images = &entry.images;
            let attachments = &entry.attachments;
            let inter_agent = entry.inter_agent.as_ref();
            let subagent_terminal = entry.subagent_terminal.as_ref();
            if let Some(terminal) = subagent_terminal {
                terminal_notice(terminal, &text, t).into_any_element()
            } else if actor == BlockActor::System && text.starts_with("<qaqh_goal_continuation") {
                notice_line("目标续跑", &text, t.warn, t).into_any_element()
            } else if let Some(envelope) = inter_agent {
                notice_line(
                    &format!("跨代理消息 · 来自 {}", envelope.author),
                    &text,
                    t.accent,
                    t,
                )
                .into_any_element()
            } else if actor != BlockActor::User {
                let label = match actor {
                    BlockActor::System => "系统插话",
                    BlockActor::Api => "API 插话",
                    BlockActor::Agent => "代理消息",
                    BlockActor::Subagent => "子代理消息",
                    BlockActor::User => unreachable!("handled above"),
                };
                notice_line(label, &text, t.dim, t).into_any_element()
            } else {
                let mut row = match purpose {
                    InputPurpose::TriggerTurn => div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap_1p5()
                        .child(user_bubble(
                            SharedString::from(format!("user-msg-{index}")),
                            &text,
                            entity,
                            t,
                        )),
                    InputPurpose::QueueOnly => notice_line("排队消息", &text, t.dim, t),
                    InputPurpose::Steer => notice_line("运行中转向", &text, t.warn, t),
                    InputPurpose::Interject => notice_line("运行中插话", &text, t.warn, t),
                };
                if !images.is_empty() {
                    row = row.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(images.iter().map(|image| {
                                qaqh_gpui_render::media::preview(
                                    image,
                                    store.blobs.get(&image.hash),
                                )
                            })),
                    );
                }
                if !attachments.is_empty() {
                    row = row.child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(format!("{} 个文本附件", attachments.len())),
                    );
                }
                if matches!(purpose, InputPurpose::TriggerTurn)
                    && !store.is_running()
                    && let Some(tid) = turn_id
                {
                    let tid_str = tid.as_str().to_owned();
                    let confirming =
                        store.confirm_truncate_turn.as_deref() == Some(tid_str.as_str());
                    let group_id = SharedString::from(format!("user-turn-{tid_str}"));
                    let weak_undo = entity.downgrade();
                    let text_owned = text.clone();
                    let bar = if confirming {
                        let tid_confirm = tid_str.clone();
                        let weak_confirm = weak_undo.clone();
                        let weak_cancel = weak_undo;
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id(SharedString::from(format!("turn-trunc-ok-{tid_str}")))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(RADIUS_CHIP))
                                    .bg(soft(t.bad, 0.12))
                                    .text_xs()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.bad)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(soft(t.bad, 0.20)))
                                    .on_click(move |_, window, cx| {
                                        let tid = tid_confirm.clone();
                                        let prefill = text_owned.clone();
                                        let _ = weak_confirm.update(cx, |store, cx| {
                                            if let Ok(turn_id) = TurnId::new(tid) {
                                                store.truncate_turn(
                                                    turn_id,
                                                    Some(prefill),
                                                    window,
                                                    cx,
                                                );
                                            }
                                        });
                                    })
                                    .child("确认从此处截断？"),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("turn-trunc-no-{tid_str}")))
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(RADIUS_CHIP))
                                    .text_xs()
                                    .text_color(t.dim)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover).text_color(t.text))
                                    .on_click(move |_, _, cx| {
                                        let _ = weak_cancel.update(cx, |store, cx| {
                                            store.confirm_truncate_turn = None;
                                            cx.notify();
                                        });
                                    })
                                    .child("取消"),
                            )
                    } else {
                        let tid_arm = tid_str.clone();
                        div()
                            .flex()
                            .items_center()
                            .opacity(0.0)
                            .group_hover(group_id.clone(), |s| s.opacity(1.0))
                            .child(
                                div()
                                    .id(SharedString::from(format!("turn-trunc-arm-{tid_str}")))
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(RADIUS_CHIP))
                                    .flex()
                                    .items_center()
                                    .gap_1()
                                    .text_xs()
                                    .text_color(t.faint)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover).text_color(t.text))
                                    .on_click(move |_, _, cx| {
                                        let tid = tid_arm.clone();
                                        let _ = weak_undo.update(cx, |store, cx| {
                                            store.confirm_truncate_turn = Some(tid);
                                            cx.notify();
                                        });
                                    })
                                    .child(Icon::new(IconName::Undo2).with_size(Size::XSmall))
                                    .child("撤回并重编辑"),
                            )
                    };
                    div()
                        .id(SharedString::from(format!("user-turn-wrap-{}", tid.as_str())))
                        .group(group_id)
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap_1()
                        .child(row)
                        .child(bar)
                        .into_any_element()
                } else {
                    row.into_any_element()
                }
            }
        }
        BlockKind::Answer | BlockKind::Reasoning => unreachable!("handled above"),
        BlockKind::ToolCall => {
            let call_id = entry.tool_call_id.as_ref()?;
            let tool = store.tools.iter().find(|tool| &tool.tool_call_id == call_id)?;
            tool_card(cx, store, entity, tool, t, tail_max).into_any_element()
        }
        BlockKind::System => div()
            .flex()
            .items_center()
            .gap_3()
            .py_1()
            .child(div().flex_1().h(px(1.)).bg(t.border_subtle))
            .child(div().text_xs().text_color(t.faint).child(SharedString::from(
                content_text(store, &entry.content),
            )))
            .child(div().flex_1().h(px(1.)).bg(t.border_subtle))
            .into_any_element(),
        BlockKind::ToolResult => return None,
    })
}

fn interaction_card(
    kind_label: &str,
    request: &str,
    buttons: Vec<AnyElement>,
    body: Vec<AnyElement>,
    t: &Palette,
) -> Div {
    div()
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(soft(t.warn, 0.35))
        .bg(t.warn_bg)
        .shadow(shadow_card(t))
        .px_3p5()
        .py_2p5()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .with_size(Size::XSmall)
                        .text_color(t.warn),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.warn)
                        .child(SharedString::from(format!("需要确认 · {kind_label}"))),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(t.text)
                .child(SharedString::from(request.to_string())),
        )
        .when(!body.is_empty(), |card| {
            card.child(div().flex().flex_col().gap_1().children(body))
        })
        .when(!buttons.is_empty(), |card| {
            card.child(div().flex().gap_2().children(buttons))
        })
}

impl Render for SessionStore {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.colors();
        crate::terminal_panel::sync(self, cx);

        let settings = settings::get(cx).clone();
        let ty = settings.typography();
        let tail_max = settings.behavior.output_tail_max;
        let collapse_work_trace = settings.behavior.collapse_work_trace;
        self.output_budget = tail_max;

        let projection = build_timeline_items(self, collapse_work_trace);
        if collapse_work_trace {
            let mut active_live: Vec<(String, Option<TurnId>)> = projection
                .items
                .iter()
                .filter_map(|item| match item {
                    TimelineItem::WorkTrace {
                        trace_key,
                        turn_id,
                        attach_live_tail: true,
                        ..
                    } => Some((trace_key.clone(), turn_id.clone())),
                    _ => None,
                })
                .collect();
            if !projection.live_tail_attached && has_live_foldable_tail(self) {
                active_live.push((
                    projection.live_tail_key.clone(),
                    projection.live_tail_turn_id.clone(),
                ));
            }
            let finished_keys: Vec<String> = self
                .trace_started_at
                .keys()
                .filter(|k| !active_live.iter().any(|(ak, _)| ak == *k))
                .cloned()
                .collect();
            for k in finished_keys {
                if let Some(started) = self.trace_started_at.remove(&k) {
                    self.trace_elapsed_ms
                        .insert(k, started.elapsed().as_millis() as u64);
                }
            }
            for (key, turn_id) in &active_live {
                self.trace_started_at.entry(key.clone()).or_insert_with(|| {
                    turn_id
                        .as_ref()
                        .and_then(|tid| self.turn_started_at.get(tid).copied())
                        .filter(|_| self.trace_elapsed_ms.is_empty())
                        .unwrap_or_else(Instant::now)
                });
                let handle = self
                    .trace_scroll_handles
                    .entry(key.clone())
                    .or_insert_with(ScrollHandle::new);
                if !self.collapsed_traces.contains(key) {
                    let max_y = handle.max_offset().y;
                    let off_y = handle.offset().y;
                    if max_y <= px(0.0) || off_y <= -max_y + px(28.0) {
                        handle.scroll_to_bottom();
                    }
                }
            }
            for item in &projection.items {
                if let TimelineItem::WorkTrace { trace_key, .. } = item {
                    self.trace_scroll_handles
                        .entry(trace_key.clone())
                        .or_insert_with(ScrollHandle::new);
                }
            }
        }

        let live_tail_attached = projection.live_tail_attached;
        let live_tail_key = projection.live_tail_key;
        let display_items = Arc::new(projection.items);
        let list_state = self.timeline_list.clone();
        let target_count = display_items.len() + 1;
        let current_count = list_state.item_count();
        if current_count != target_count {
            let old_history = current_count.saturating_sub(1);
            let new_history = display_items.len();
            let start = old_history.min(new_history);
            list_state.splice(start..old_history, new_history - start);
        }

        let follow = settings.behavior.stream_follow;
        if self.timeline_follow_setting != Some(follow) {
            list_state.set_follow_mode(if follow {
                FollowMode::Tail
            } else {
                FollowMode::Normal
            });
            self.timeline_follow_setting = Some(follow);
        }

        let entity = cx.entity();
        let items_for_list = display_items.clone();
        let timeline_list_el = list(list_state, move |index, window, cx| {
            let row = div().w_full().when(index == 0, |row| row.pt_4());
            let gap = px((10.0 + ty.density_shift).max(4.));

            if let Some(item) = items_for_list.get(index) {
                return match item {
                    TimelineItem::Single(entry_idx) => {
                        let (is_assistant, last_assistant) = {
                            let store = entity.read(cx);
                            let is_assistant = store
                                .entries
                                .get(*entry_idx)
                                .is_some_and(|block| {
                                    matches!(block.kind, BlockKind::Answer | BlockKind::Reasoning)
                                });
                            let last_assistant = items_for_list.iter().rev().find_map(|item| {
                                if let TimelineItem::Single(idx) = item
                                    && store.entries.get(*idx).is_some_and(|block| {
                                        matches!(
                                            block.kind,
                                            BlockKind::Answer | BlockKind::Reasoning
                                        )
                                    })
                                {
                                    Some(*idx)
                                } else {
                                    None
                                }
                            });
                            (is_assistant, last_assistant)
                        };
                        match timeline_entry(window, cx, &entity, *entry_idx, &t, &ty, tail_max) {
                            Some(content) => {
                                let wrapped = if is_assistant {
                                    content
                                } else {
                                    prose_rail(&ty).child(content).into_any_element()
                                };
                                let row = row.pb(gap).child(wrapped);
                                if last_assistant == Some(*entry_idx) {
                                    row.child(prose_rail(&ty).child(copy_conversation_chip(&entity, &t)))
                                        .into_any_element()
                                } else {
                                    row.into_any_element()
                                }
                            }
                            None => row.into_any_element(),
                        }
                    }
                    TimelineItem::ToolBurst { group_id, indices } => row
                        .pb(gap)
                        .child(prose_rail(&ty).child(tool_burst_card(
                            window, cx, &entity, *group_id, indices, &t, tail_max,
                        )))
                        .into_any_element(),
                    TimelineItem::WorkTrace {
                        trace_key,
                        indices,
                        attach_live_tail,
                        ..
                    } => row
                        .pb(gap)
                        .child(prose_rail(&ty).child(work_trace_card(
                            window,
                            cx,
                            &entity,
                            trace_key.clone(),
                            indices,
                            *attach_live_tail,
                            &t,
                            &ty,
                            tail_max,
                        )))
                        .into_any_element(),
                };
            }

            let (streaming_meta, show_unattached_live_trace) = {
                let store = entity.read(cx);
                let streaming_meta: Vec<(usize, BlockId, BlockKind)> = store
                    .streaming
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, kind, text))| {
                        has_assistant_text(*kind, text)
                            && (!collapse_work_trace || *kind == BlockKind::Answer)
                    })
                    .map(|(i, (block_id, kind, _))| (i, block_id.clone(), *kind))
                    .collect();
                let show_live_trace = collapse_work_trace
                    && !live_tail_attached
                    && has_live_foldable_tail(store);
                (streaming_meta, show_live_trace)
            };

            let live_trace_el = show_unattached_live_trace.then(|| {
                prose_rail(&ty)
                    .child(work_trace_card(
                        window,
                        cx,
                        &entity,
                        live_tail_key.clone(),
                        &[],
                        true,
                        &t,
                        &ty,
                        tail_max,
                    ))
                    .into_any_element()
            });

            let streaming_els: Vec<AnyElement> = streaming_meta
                .into_iter()
                .map(|(i, block_id, kind)| {
                    assistant_section(
                        window,
                        cx,
                        &kind,
                        AssistantSource::Streaming(i),
                        true,
                        i * 2 + 1,
                        &block_id,
                        &entity,
                        &t,
                        &ty,
                    )
                    .into_any_element()
                })
                .collect();

            let animate_motion = effective_motion(cx) != MotionLevel::Off;
            let store = entity.read(cx);
            let referenced_tools: HashSet<_> = store
                .entries
                .iter()
                .filter_map(|block| block.tool_call_id.as_ref())
                .collect();

            let arg_stream_els: Vec<AnyElement> = if collapse_work_trace {
                Vec::new()
            } else {
                store
                    .arg_stream
                    .iter()
                    .map(|stream| {
                        prose_rail(&ty)
                            .child(render_arg_stream_item(
                                store,
                                &entity,
                                stream,
                                animate_motion,
                                &t,
                            ))
                            .into_any_element()
                    })
                    .collect()
            };

            let unref_tool_els: Vec<AnyElement> = store
                .tools
                .iter()
                .filter_map(|tool| {
                    if referenced_tools.contains(&tool.tool_call_id) {
                        return None;
                    }
                    if collapse_work_trace && is_trace_foldable_tool_view(store, tool) {
                        return None;
                    }
                    Some(
                        prose_rail(&ty)
                            .child(tool_card(cx, store, &entity, tool, &t, tail_max))
                            .into_any_element(),
                    )
                })
                .collect();

            div()
                .w_full()
                .when(index == 0, |row| row.pt_4())
                .pb_6()
                .flex()
                .flex_col()
                .gap(gap)
                .children(live_trace_el)
                .children(streaming_els)
                .children(arg_stream_els)
                .children(unref_tool_els)
                .children(store.subagents.iter().map(|subagent| {
                    let status = format!("{:?}", subagent.status);
                    let row = div()
                        .px_3()
                        .py_2()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(t.surface_subtle)
                        .border_1()
                        .border_color(t.border_subtle)
                        .flex()
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(t.dim)
                        .child(Icon::new(IconName::Bot).with_size(Size::XSmall).text_color(t.accent))
                        .child(SharedString::from(format!(
                            "子代理 {} · {status} · {}",
                            subagent.session_id.as_str(),
                            subagent
                                .nickname
                                .as_deref()
                                .unwrap_or("未命名"),
                        )));
                    prose_rail(&ty).child(row).into_any_element()
                }))
                .into_any_element()
        })
        .flex_1()
        .min_h_0()
        .w_full();

        let timeline = div()
            .id("timeline-panel")
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .on_mouse_move(cx.listener(|store, event: &MouseMoveEvent, _, cx| {
                let bounds = store.timeline_list.viewport_bounds();
                if f32::from(bounds.size.width) <= 0.0 {
                    return;
                }
                let rel_x = f32::from(event.position.x - bounds.origin.x);
                let threshold = if store.turn_rail_near_left {
                    104.0
                } else {
                    72.0
                };
                let near = rel_x >= 0.0 && rel_x <= threshold;
                if store.turn_rail_near_left != near {
                    store.turn_rail_near_left = near;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|store, hovered: &bool, _, cx| {
                if !*hovered && store.turn_rail_near_left {
                    store.turn_rail_near_left = false;
                    cx.notify();
                }
            }))
            .vertical_scrollbar(&self.timeline_list)
            .child(timeline_list_el)
            .child(
                bottom_fade_mask(t.stage, 20.)
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0(),
            )
            .children(todo_floating_card(self, cx, &t))
            .children(crate::turn_rail::render(
                self,
                &display_items,
                &t,
                window,
                cx,
            ))
            .children(scroll_to_latest_button(self, &t, window, cx));

        let mut pending_interactions = Vec::new();
        for interaction in self.interactions.clone().iter() {
            match interaction.kind {
                InteractionKind::Permission => {
                    // V3 按钮组由后端直供(`choices`),前端不许自己猜。
                    let interaction_id = interaction.interaction_id.clone();
                    let uid = interaction.interaction_id.as_str().to_owned();
                    let mut buttons: Vec<AnyElement> = Vec::new();
                    for (choice_index, choice) in interaction.choices.iter().enumerate() {
                        let resolution = match choice.decision {
                            InteractionDecisionKind::Approved => {
                                Some(CommandInteractionResolution::Approve {
                                    scope: PermissionGrantScope::Once,
                                })
                            }
                            InteractionDecisionKind::ApprovedForSession => {
                                Some(CommandInteractionResolution::Approve {
                                    scope: PermissionGrantScope::Session,
                                })
                            }
                            InteractionDecisionKind::Rejected => {
                                Some(CommandInteractionResolution::Reject {
                                    text: String::new(),
                                })
                            }
                            InteractionDecisionKind::Answered
                            | InteractionDecisionKind::Dismissed => None,
                        };
                        let Some(resolution) = resolution else { continue };
                        let color = match choice.tone {
                            ChoiceTone::Primary => t.accent,
                            ChoiceTone::Secondary => t.ok,
                            ChoiceTone::Destructive => t.bad,
                        };
                        let target = interaction_id.clone();
                        let listener = cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_interaction(&target, resolution.clone());
                                cx.notify();
                            },
                        );
                        buttons.push(
                            action_button(
                                cx,
                                format!("{uid}-choice-{choice_index}"),
                                choice.label.clone(),
                                color,
                                listener,
                                &t,
                            )
                            .into_any_element(),
                        );
                    }
                    let mut body: Vec<AnyElement> = Vec::new();
                    if let Some(permission) = &interaction.permission {
                        let mut meta = vec![permission.category.clone(), permission.risk.clone()];
                        if let Some(summary) = &permission.action_summary {
                            meta.push(summary.clone());
                        }
                        body.push(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .child(SharedString::from(meta.join(" · ")))
                                .into_any_element(),
                        );
                        if !permission.paths.is_empty() {
                            body.push(
                                div()
                                    .text_xs()
                                    .text_color(t.faint)
                                    .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                                    .child(SharedString::from(permission.paths.join("\n")))
                                    .into_any_element(),
                            );
                        }
                    }
                    pending_interactions.push(
                        interaction_card(
                            &interaction.title,
                            &interaction.body,
                            buttons,
                            body,
                            &t,
                        )
                        .into_any_element(),
                    );
                }
                InteractionKind::Ask => {
                    pending_interactions.push(crate::ask_dock::render(
                        self,
                        interaction,
                        window,
                        cx,
                        &t,
                    ));
                }
            }
        }

        let interactions_box = (!pending_interactions.is_empty()).then(|| {
            div()
                .w_full()
                .max_w(px(MESSAGE_MAX_WIDTH_PX))
                .mx_auto()
                .px_4()
                .pb_2()
                .flex()
                .flex_col()
                .gap_2()
                .children(pending_interactions)
        });

        let composer = crate::composer::render(self, cx, &t);

        let conversation = qaqh_gpui_kit::kit::stage_surface(&t)
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .children(crate::controls::search(self, cx))
            .child(timeline)
            .children(provider_retry_banner(self, &t))
            .children(crate::code_panel::review_button(self, &t, cx))
            .children(interactions_box)
            .child(composer);

        div()
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(conversation)
            .children(crate::code_panel::render(self, &t, &ty, window, cx))
            .children(crate::terminal_panel::render(self, &t, &ty, window, cx))
            .children(crate::composer::model_params_modal(self, cx, &t))
    }
}
