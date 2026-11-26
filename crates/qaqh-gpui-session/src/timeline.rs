//! 时间线渲染(DESIGN LAW v3.0 · v5.0.0):
//! - 纯净悬浮主舞台:去掉冗余会话头,专注叙事阅读与交互;
//! - Antigravity 无框轨迹脊柱(ToolBurst / Thinking):左侧 1px 脊柱 + 物理弹簧 MotionReveal;
//! - 中性扫光思考态 + 底部 20px 渐隐过渡 + 无空白常驻 footer。

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Context, Div, Entity, FollowMode,
    FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, list, px, relative,
};
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_ringing_contract::fact::{
    ActorKind, AssistantBlockKind, BlockId, ContentValue, InputPurpose, InteractionKind,
    PermissionGrantScope, ToolCallId,
};
use qaqh_ringing_contract::state::{
    ExecSessionState, TimelineEntry, ToolCallState, ToolCallView,
};
use qaqh_types::{SessionRunStatus, ToolCallOutcome};

use qaqh_gpui_kit::kit::{
    RADIUS_BUBBLE, RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, shadow_card, shadow_popover, soft,
};
use qaqh_gpui_kit::motion::{
    accordion_reveal, bottom_fade_mask, effective_motion, live_dot, shimmer_label, spring_chevron,
};
use qaqh_gpui_kit::primitives::action_button;
use qaqh_gpui_kit::settings::{self, MotionLevel};
use qaqh_gpui_kit::theme::{ActiveTheme, Palette};

use crate::store::{ArgStream, SessionStore};

/// 自动换行悬停提示卡:用于单行截断的 Todo 条目,悬停展示完整多行内容。
struct WrappingTooltip {
    text: SharedString,
}

impl Render for WrappingTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.colors();
        div()
            .max_w(px(320.))
            .px_2p5()
            .py_1p5()
            .rounded(px(RADIUS_CHIP))
            .bg(t.card)
            .border_1()
            .border_color(t.border)
            .shadow(shadow_popover(&t))
            .text_xs()
            .text_color(t.text)
            .child(self.text.clone())
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

    for (idx, item) in store.todos.iter().enumerate() {
        let in_progress = item.status == "in_progress";
        let settled = item.status == "completed" || item.status == "cancelled";
        let icon_el: AnyElement = match item.status.as_str() {
            "completed" => Icon::new(IconName::CircleCheck)
                .with_size(Size::XSmall)
                .text_color(t.ok)
                .into_any_element(),
            "cancelled" => Icon::new(IconName::CircleX)
                .with_size(Size::XSmall)
                .text_color(t.bad)
                .into_any_element(),
            "in_progress" => Icon::new(IconName::LoaderCircle)
                .with_size(Size::XSmall)
                .text_color(t.accent)
                .with_animation(
                    SharedString::from(format!("floating-todo-spin-{idx}")),
                    Animation::new(std::time::Duration::from_millis(900)).repeat(),
                    |icon, val| icon.rotate(gpui_kit::Radians(val * std::f32::consts::TAU)),
                )
                .into_any_element(),
            _ => Icon::new(IconName::Circle)
                .with_size(Size::XSmall)
                .text_color(t.faint)
                .into_any_element(),
        };
        let full_title = SharedString::from(item.title.clone());
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
                    let text = full_title.clone();
                    cx.new(move |_| WrappingTooltip { text }).into()
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

pub(crate) fn content_text(value: &ContentValue) -> &str {
    match value {
        ContentValue::Inline { text } => text,
        ContentValue::Ref { .. } => "引用内容（未加载）",
        ContentValue::Unavailable(_) => "内容不可用",
    }
}

/// 视图投影条目:单条或连续成功工具归组(不改变 entries 数据真值)。
pub(crate) enum TimelineItem {
    Single(usize),
    ToolBurst {
        group_id: usize,
        indices: Vec<usize>,
    },
}

fn is_groupable_tool(store: &SessionStore, entry: &TimelineEntry) -> bool {
    let TimelineEntry::ToolCall { call_id, .. } = entry else {
        return false;
    };
    let Some(tool) = store.tools.iter().find(|tool| &tool.call_id == call_id) else {
        return false;
    };
    if crate::store::handoff_facts(tool).is_some() {
        return false;
    }
    matches!(
        &tool.state,
        ToolCallState::Finished { outcome } if outcome.is_success()
    )
}

pub(crate) fn build_timeline_items(store: &SessionStore) -> Vec<TimelineItem> {
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
        if hidden_timeline_entry(entry) {
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
    items
}

/// 搜索跳转索引映射:原始 entry 下标 → 当前投影列表项下标。
pub(crate) fn display_index_for_entry(store: &SessionStore, entry_index: usize) -> usize {
    let items = build_timeline_items(store);
    items
        .iter()
        .position(|item| match item {
            TimelineItem::Single(index) => *index == entry_index,
            TimelineItem::ToolBurst { indices, .. } => indices.contains(&entry_index),
        })
        .unwrap_or(0)
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

fn state_label(state: &ToolCallState) -> &'static str {
    match state {
        ToolCallState::Declared => "已就绪",
        ToolCallState::AwaitingInteraction { .. } => "等待确认",
        ToolCallState::Running => "执行中",
        ToolCallState::Cancelling => "取消中",
        ToolCallState::Finished { outcome } => match outcome {
            ToolCallOutcome::Completed => "已完成",
            ToolCallOutcome::Yielded { .. } => "后台运行",
            ToolCallOutcome::Failed { .. } => "失败",
            ToolCallOutcome::Cancelled { .. } => "已取消",
            ToolCallOutcome::Interrupted { .. } => "已中断",
        },
    }
}

fn state_color(state: &ToolCallState, t: &Palette) -> Hsla {
    match state {
        ToolCallState::Declared => t.dim,
        ToolCallState::AwaitingInteraction { .. } => t.warn,
        ToolCallState::Running => t.accent,
        ToolCallState::Cancelling => t.warn,
        ToolCallState::Finished { outcome } => match outcome {
            ToolCallOutcome::Completed => t.ok,
            ToolCallOutcome::Yielded { .. } => t.accent,
            ToolCallOutcome::Failed { .. } => t.bad,
            ToolCallOutcome::Cancelled { .. } | ToolCallOutcome::Interrupted { .. } => t.dim,
        },
    }
}

fn exec_state_label(state: &ExecSessionState, t: &Palette) -> (&'static str, Hsla) {
    match state {
        ExecSessionState::Running => ("运行中", t.accent),
        ExecSessionState::Stopping { .. } => ("停止中", t.warn),
        ExecSessionState::Finished { .. } => ("已结束", t.dim),
    }
}

/// 会话运行状态的语义色:运行 accent,等待/取消 warn,空闲/未运行 dim。
pub fn status_color(status: SessionRunStatus, t: &Palette) -> Hsla {
    match status {
        SessionRunStatus::Running => t.accent,
        SessionRunStatus::WaitingUser | SessionRunStatus::Cancelling => t.warn,
        SessionRunStatus::Idle | SessionRunStatus::NotRunning => t.dim,
    }
}

/// 侧栏状态点:运行/待响应/取消中高亮,空闲置灰,未加载不显示。
pub fn status_dot(status: SessionRunStatus, t: &Palette) -> Div {
    let color = status_color(status, t);
    let label = match status {
        SessionRunStatus::Running => "运行中",
        SessionRunStatus::WaitingUser => "待响应",
        SessionRunStatus::Cancelling => "取消中",
        SessionRunStatus::Idle => "空闲",
        SessionRunStatus::NotRunning => "",
    };
    if label.is_empty() {
        return div();
    }
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

fn tool_summary(name: &str, state: &ToolCallState) -> Option<&'static str> {
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
    match state {
        ToolCallState::Finished { outcome } => outcome.is_success().then_some(done),
        ToolCallState::Declared | ToolCallState::Running => Some(active),
        ToolCallState::AwaitingInteraction { .. } if name == "ask" => Some(active),
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
        let Some(TimelineEntry::ToolCall { call_id, .. }) = store.entries.get(idx) else {
            continue;
        };
        let Some(tool) = store.tools.iter().find(|t| &t.call_id == call_id) else {
            others += 1;
            continue;
        };
        let name = tool.tool_name.as_deref().unwrap_or("");
        let is_exec = name == "exec_command"
            || store.exec_sessions.iter().any(|e| e.origin_call_id == tool.call_id);
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

fn tool_status_line(name: &str, title: String, state: &ToolCallState, t: &Palette) -> Div {
    let icon = tool_category_icon(name, false);
    let color = state_color(state, t);
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
fn tool_status_icon(call_id: &ToolCallId, state: &ToolCallState, t: &Palette) -> AnyElement {
    let (name, spin) = match state {
        ToolCallState::Finished { outcome } if outcome.is_success() => {
            (IconName::CircleCheck, false)
        }
        ToolCallState::Finished { .. } => (IconName::CircleAlert, false),
        ToolCallState::Declared => (IconName::LoaderCircle, false),
        _ => (IconName::LoaderCircle, true),
    };
    let icon = Icon::new(name)
        .with_size(Size::XSmall)
        .text_color(state_color(state, t));
    if spin {
        icon.with_animation(
            SharedString::from(format!("tool-spin-{}", call_id.0)),
            Animation::new(std::time::Duration::from_millis(900)).repeat(),
            |icon, value| icon.rotate(gpui_kit::Radians(value * std::f32::consts::TAU)),
        )
        .into_any_element()
    } else {
        icon.into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn tool_toggle(
    entity: &Entity<SessionStore>,
    call_id: &ToolCallId,
    expanded: bool,
    status: AnyElement,
    category: IconName,
    title: String,
    delta: Option<(usize, usize)>,
    animate_motion: bool,
    t: &Palette,
) -> gpui_kit::Stateful<Div> {
    let entity = entity.downgrade();
    let call_id = call_id.clone();
    div()
        .id(SharedString::from(format!("tool-toggle-{}", call_id.0)))
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
        .child(
            Icon::new(category)
                .with_size(Size::XSmall)
                .text_color(t.dim),
        )
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
            SharedString::from(format!("tool-chev-{}", call_id.0)),
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

fn tool_card(
    cx: &gpui_kit::App,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    tool: &ToolCallView,
    t: &Palette,
    tail_max: usize,
) -> Div {
    let name = tool.tool_name.as_deref().unwrap_or("(未知工具)");
    let exec = store
        .exec_sessions
        .iter()
        .find(|exec| exec.origin_call_id == tool.call_id);
    let is_exec = exec.is_some() || name == "exec_command";
    let command = if let Some(exec) = exec {
        Some(exec.command.as_str())
    } else if is_exec {
        tool.args.as_ref().and_then(|args| args.get("cmd")?.as_str())
    } else {
        None
    };
    let (status, status_color) = if let Some(exec) = exec {
        exec_state_label(&exec.state, t)
    } else {
        (state_label(&tool.state), state_color(&tool.state, t))
    };
    let fork_suffix = fork_suffix(name, tool.args.as_ref());
    let title = if let Some(command) = command {
        let command = command.replace(['\r', '\n'], " ");
        format!("$ {command}  ·  {status}")
    } else if let Some(summary) = tool_summary(name, &tool.state) {
        format!("{summary}{fork_suffix}")
    } else {
        format!("{name}  ·  {status}{fork_suffix}")
    };
    if tool_status_only(name) {
        return tool_status_line(name, title, &tool.state, t);
    }
    let expanded = store.expanded_tools.contains(&tool.call_id);
    let delta = tool
        .code_delta
        .as_ref()
        .map(|d| (d.lines_added, d.lines_removed));
    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    let header = tool_toggle(
        entity,
        &tool.call_id,
        expanded,
        tool_status_icon(&tool.call_id, &tool.state, t),
        tool_category_icon(name, is_exec),
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
    if !is_exec && let Some(args) = &tool.args {
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
    let raw_output = if let Some(exec) = exec {
        store
            .exec_output_tail
            .iter()
            .find(|(id, end, text)| id == &exec.id && *end > exec.next_offset && !text.is_empty())
            .map_or(exec.output_preview.as_str(), |(_, _, text)| text.as_str())
    } else if matches!(tool.state, ToolCallState::Finished { .. }) {
        tool.output.as_ref().map_or("", content_text)
    } else {
        store
            .output_tail
            .iter()
            .find(|(call_id, _)| call_id == &tool.call_id)
            .map_or("", |(_, tail)| tail.as_str())
    };
    let result = if is_exec && exec.is_none() {
        serde_json::from_str::<serde_json::Value>(raw_output).ok()
    } else {
        None
    };
    let output = result
        .as_ref()
        .and_then(|result| result.get("output")?.as_str())
        .unwrap_or(raw_output);
    let (output, truncated) = utf8_tail(output, tail_max);
    if !output.is_empty() {
        body = body.child(
            div()
                .id(SharedString::from(format!("tool-output-{}", tool.call_id.0)))
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
        && !exec.state.is_terminal()
    {
        let id = exec.id.0.clone();
        let control = |terminate| {
            let entity = entity.downgrade();
            let exec_id = id.clone();
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
        let exec_id = exec.id.0.clone();
        let exec_handle = exec.id.clone();
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
                if let Some(TimelineEntry::ToolCall { call_id, .. }) = store.entries.get(idx)
                    && let Some(tool) = store.tools.iter().find(|tool| &tool.call_id == call_id)
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

fn has_assistant_text(kind: AssistantBlockKind, text: &str) -> bool {
    kind != AssistantBlockKind::ToolCall && !text.trim().is_empty()
}

fn hidden_timeline_entry(entry: &TimelineEntry) -> bool {
    match entry {
        TimelineEntry::ToolResult { .. } => true,
        TimelineEntry::AssistantBlock {
            block_kind,
            content,
            ..
        } => !has_assistant_text(*block_kind, content_text(content)),
        _ => false,
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
            format!("thinking-shimmer-{}", block_id.0),
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
        .id(SharedString::from(format!("thinking-toggle-{}", block_id.0)))
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
            SharedString::from(format!("think-chev-{}", block_id.0)),
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

fn assistant_source_text<'a>(store: &'a SessionStore, source: AssistantSource) -> &'a str {
    match source {
        AssistantSource::Entry(idx) => match store.entries.get(idx) {
            Some(TimelineEntry::AssistantBlock { content, .. }) => content_text(content),
            _ => "",
        },
        AssistantSource::Streaming(idx) => store
            .streaming
            .get(idx)
            .map(|(_, _, text)| text.as_str())
            .unwrap_or(""),
    }
}

/// 标准正文阅读轨(与原对话宽度像素一致),表格破栏块以外的所有元素统一走此轨道居中。
fn prose_rail(ty: &settings::Typography) -> Div {
    div()
        .w_full()
        .max_w(px(ty.content_width + crate::markdown::PROSE_RAIL_PAD_PX))
        .mx_auto()
        .px_4()
}

/// 助手块 markdown 区(Reasoning 使用无框思考行 + 左侧 1px 脊柱 MotionReveal 展开)。
#[allow(clippy::too_many_arguments)]
fn assistant_section(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    kind: &AssistantBlockKind,
    source: AssistantSource,
    live: bool,
    section: usize,
    block_id: &BlockId,
    entity: &Entity<SessionStore>,
    t: &Palette,
    ty: &settings::Typography,
) -> AnyElement {
    let animate_motion = effective_motion(cx) != MotionLevel::Off;
    let reasoning = matches!(kind, AssistantBlockKind::Reasoning);
    let weak_entity = entity.downgrade();
    let (expanded, header, body, full_text, md_extensions, md_key) = {
        let store = entity.read(cx);
        let text = assistant_source_text(store, source);
        let md_key = SharedString::from(format!(
            "md-{}-{}-{section}",
            store.session_id, block_id.0
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
        } else if matches!(kind, AssistantBlockKind::ToolCall) {
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
                text,
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
            SharedString::from(format!("think-reveal-{}", block_id.0)),
            expanded,
            window,
            cx,
            move |cx| {
                let store = entity_for_body.read(cx);
                let text = assistant_source_text(store, source);
                let body = crate::markdown::render_document(
                    md_key_for_body.clone(),
                    text,
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
                    .child(live_dot(format!("live-gen-{}", block_id.0), t.accent, true))
                    .child(shimmer_label(
                        format!("live-shimmer-{}", block_id.0),
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
    terminal: &qaqh_ringing_contract::commands::SubagentTerminalNotification,
    text: &str,
    t: &Palette,
) -> Div {
    use qaqh_ringing_contract::commands::SubagentTerminalKind;
    let (label, color) = match terminal.terminal {
        SubagentTerminalKind::Completed => ("子代理已完成", t.ok),
        SubagentTerminalKind::Failed => ("子代理失败", t.bad),
        SubagentTerminalKind::Cancelled => ("子代理已取消", t.dim),
        SubagentTerminalKind::TimedOut => ("子代理超时", t.warn),
    };
    let mut title = format!("{label} · {}", terminal.child_session_id);
    if !terminal.parent_call_id.is_empty() {
        title.push_str(&format!(" (调用 {})", terminal.parent_call_id));
    }
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
            TimelineEntry::AssistantBlock {
                block_id,
                block_kind,
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
        if matches!(store.entries.get(index), Some(TimelineEntry::Input { .. })) {
            crate::handoff::detect_open_source(store, index).map(|src| {
                let text = match store.entries.get(index) {
                    Some(TimelineEntry::Input { content, .. }) => content_text(content).to_owned(),
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
        if let Some(TimelineEntry::ToolCall { call_id, .. }) = store.entries.get(index) {
            store
                .tools
                .iter()
                .find(|tool| &tool.call_id == call_id)
                .and_then(crate::store::handoff_facts)
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
    Some(match entry {
        TimelineEntry::Input {
            turn_id,
            content,
            images,
            attachments,
            inter_agent,
            subagent_terminal,
            actor,
            purpose,
            ..
        } => {
            let text = content_text(content);
            if let Some(terminal) = subagent_terminal {
                terminal_notice(terminal, text, t).into_any_element()
            } else if actor.kind == ActorKind::System && text.starts_with("<qaqh_goal_continuation") {
                notice_line("目标续跑", text, t.warn, t).into_any_element()
            } else if let Some(envelope) = inter_agent {
                notice_line(
                    &format!("跨代理消息 · 来自 {}", envelope.author),
                    text,
                    t.accent,
                    t,
                )
                .into_any_element()
            } else if actor.kind != ActorKind::User {
                let label = match actor.kind {
                    ActorKind::System => "系统插话",
                    ActorKind::Api => "API 插话",
                    ActorKind::Agent => "代理消息",
                    ActorKind::Subagent => "子代理消息",
                    ActorKind::User => unreachable!("handled above"),
                };
                notice_line(label, text, t.dim, t).into_any_element()
            } else {
                let mut row = match purpose {
                    InputPurpose::TriggerTurn => div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap_1p5()
                        .child(user_bubble(
                            SharedString::from(format!("user-msg-{index}")),
                            text,
                            entity,
                            t,
                        )),
                    InputPurpose::QueueOnly => notice_line("排队消息", text, t.dim, t),
                    InputPurpose::Steer => notice_line("运行中转向", text, t.warn, t),
                    InputPurpose::Interject => notice_line("运行中插话", text, t.warn, t),
                };
                if !images.is_empty() {
                    row = row.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .children(images.iter().map(qaqh_gpui_render::media::preview)),
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
                    let tid_str = tid.0.clone();
                    let confirming =
                        store.confirm_truncate_turn.as_deref() == Some(tid_str.as_str());
                    let group_id = SharedString::from(format!("user-turn-{tid_str}"));
                    let weak_undo = entity.downgrade();
                    let text_owned = text.to_owned();
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
                                            store.truncate_turn(tid, Some(prefill), window, cx);
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
                        .id(SharedString::from(format!("user-turn-wrap-{}", tid.0)))
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
        TimelineEntry::AssistantBlock { .. } => unreachable!("handled above"),
        TimelineEntry::ToolCall { call_id, .. } => {
            let tool = store.tools.iter().find(|tool| &tool.call_id == call_id)?;
            tool_card(cx, store, entity, tool, t, tail_max).into_any_element()
        }
        TimelineEntry::Compaction { .. } => div()
            .flex()
            .items_center()
            .gap_3()
            .py_1()
            .child(div().flex_1().h(px(1.)).bg(t.border_subtle))
            .child(div().text_xs().text_color(t.faint).child("上下文已自动整理压缩"))
            .child(div().flex_1().h(px(1.)).bg(t.border_subtle))
            .into_any_element(),
        TimelineEntry::ToolResult { .. } => return None,
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
        self.output_budget = tail_max;

        let display_items = Arc::new(build_timeline_items(self));
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
                            let is_assistant = matches!(
                                store.entries.get(*entry_idx),
                                Some(TimelineEntry::AssistantBlock { .. })
                            );
                            let last_assistant = store
                                .entries
                                .iter()
                                .rposition(|entry| matches!(entry, TimelineEntry::AssistantBlock { .. }));
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
                };
            }

            let streaming_meta: Vec<(usize, BlockId, AssistantBlockKind)> = {
                let store = entity.read(cx);
                store
                    .streaming
                    .iter()
                    .enumerate()
                    .filter(|(_, (_, kind, text))| has_assistant_text(*kind, text))
                    .map(|(i, (block_id, kind, _))| (i, block_id.clone(), *kind))
                    .collect()
            };
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
                .filter_map(|entry| match entry {
                    TimelineEntry::ToolCall { call_id, .. } => Some(call_id),
                    _ => None,
                })
                .collect();

            div()
                .w_full()
                .when(index == 0, |row| row.pt_4())
                .pb_6()
                .flex()
                .flex_col()
                .gap(gap)
                .children(streaming_els)
                .children(store.arg_stream.iter().map(|stream| {
                    let ArgStream {
                        call_id,
                        tool_name,
                        text,
                        lines,
                        target_files,
                        args_complete,
                    } = stream;
                    if tool_status_only(tool_name) {
                        let title = tool_summary(tool_name, &ToolCallState::Declared)
                            .map_or_else(|| format!("正在准备 {tool_name}"), str::to_owned);
                        return prose_rail(&ty)
                            .child(tool_status_line(tool_name, title, &ToolCallState::Declared, &t))
                            .into_any_element();
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
                    let expanded = store.expanded_tools.contains(call_id);
                    prose_rail(&ty)
                        .child(
                            div()
                                .child(tool_toggle(
                                    &entity,
                                    call_id,
                                    expanded,
                                    tool_status_icon(call_id, &ToolCallState::Running, &t),
                                    tool_category_icon(tool_name, tool_name == "exec_command"),
                                    title,
                                    None,
                                    animate_motion,
                                    &t,
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
                                        .id(SharedString::from(format!("arg-stream-{}", call_id.0)))
                                        .overflow_y_scroll()
                                        .child(SharedString::from(text.clone()))
                                })),
                        )
                        .into_any_element()
                }))
                .children(store.tools.iter().filter_map(|tool| {
                    if referenced_tools.contains(&tool.call_id) {
                        return None;
                    }
                    Some(
                        prose_rail(&ty)
                            .child(tool_card(cx, store, &entity, tool, &t, tail_max))
                            .into_any_element(),
                    )
                }))
                .children(store.subagents.iter().map(|subagent| {
                    let status = subagent
                        .status
                        .as_ref()
                        .map(|status| format!("{status:?}"))
                        .unwrap_or_else(|| "进行中".into());
                    let mut row = div()
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
                            "子代理 {} · {status}",
                            subagent.child_session_id.0,
                        )));
                    if let Some(result) = &subagent.result {
                        let head: String = content_text(result).chars().take(160).collect();
                        row = row.child(div().text_xs().text_color(t.dim).truncate().child(head));
                    }
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
            .vertical_scrollbar(&self.timeline_list)
            .child(timeline_list_el)
            .child(
                bottom_fade_mask(t.stage, 20.)
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0(),
            )
            .children(todo_floating_card(self, cx, &t));

        let mut pending_interactions = Vec::new();
        for interaction in self.interactions.clone().iter() {
            if interaction.resolution.is_some() {
                continue;
            }
            match interaction.kind {
                InteractionKind::Permission => {
                    let Some(call_id) = &interaction.call_id else {
                        continue;
                    };
                    let call = call_id.0.clone();
                    let allow = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(
                                    call.clone(),
                                    true,
                                    Some(PermissionGrantScope::Once),
                                );
                                cx.notify();
                            },
                        )
                    };
                    let allow_session = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(
                                    call.clone(),
                                    true,
                                    Some(PermissionGrantScope::Session),
                                );
                                cx.notify();
                            },
                        )
                    };
                    let trust = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(
                                    call.clone(),
                                    true,
                                    Some(PermissionGrantScope::Persistent),
                                );
                                cx.notify();
                            },
                        )
                    };
                    let deny = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(call.clone(), false, None);
                                cx.notify();
                            },
                        )
                    };
                    let uid = interaction.interaction_id.0.clone();
                    pending_interactions.push(
                        interaction_card(
                            "工具权限",
                            content_text(&interaction.request),
                            vec![
                                action_button(
                                    cx,
                                    format!("{uid}-allow"),
                                    "允许一次",
                                    t.ok,
                                    allow,
                                    &t,
                                )
                                .into_any_element(),
                                action_button(
                                    cx,
                                    format!("{uid}-session"),
                                    "本会话允许",
                                    t.accent,
                                    allow_session,
                                    &t,
                                )
                                .into_any_element(),
                                action_button(
                                    cx,
                                    format!("{uid}-trust"),
                                    "始终允许",
                                    t.ok,
                                    trust,
                                    &t,
                                )
                                .into_any_element(),
                                action_button(
                                    cx,
                                    format!("{uid}-deny"),
                                    "拒绝",
                                    t.bad,
                                    deny,
                                    &t,
                                )
                                .into_any_element(),
                            ],
                            Vec::new(),
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
                .max_w(px(ty.content_width + 48.))
                .mx_auto()
                .px_4()
                .pb_2()
                .flex()
                .flex_col()
                .gap_2()
                .children(pending_interactions)
        });

        let composer = crate::composer::render(self, cx, &t, ty.content_width);

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
            .children(crate::code_panel::render(self, &t, &ty, cx))
            .children(crate::terminal_panel::render(self, &t, &ty, window, cx))
            .children(crate::composer::model_params_modal(self, cx, &t))
    }
}
