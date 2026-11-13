//! 元素构建(M3):主题 token 化 + 分组侧栏/时间线/composer/交互应答卡/todo 右栏。
//! `Entity<T: Render>` 即元素:DaemonStore 为根,SessionStore 经实体直接挂载。
//! 颜色一律取 theme::Palette 语义槽;输入框用 gpui-component 的 Input/Textarea
//! (状态实体 + 组件渲染,IME/选区/剪贴板由引擎承担)。

use std::collections::HashSet;

use gpui::prelude::*;
use gpui::AnyElement;
use gpui::ClickEvent;
use gpui::Context;
use gpui::Div;
use gpui::Entity;
use gpui::FontWeight;
use gpui::Hsla;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::KeyDownEvent;
use gpui::ParentElement;
use gpui::Render;
use gpui::FollowMode;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::div;
use gpui::list;
use gpui::px;
use gpui_component::button::ButtonVariants;
use gpui_component::dialog::Dialog;
use gpui_component::input::{Input, Textarea};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Sizable, WindowExt};
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::ActorKind;
use qaqh_ringing_contract::fact::InputPurpose;
use qaqh_ringing_contract::fact::ContentValue;
use qaqh_ringing_contract::fact::InteractionKind;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::state::ExecSessionState;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallState;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_types::ToolCallOutcome;

use qaqh_gpui_wire::ask;
use qaqh_gpui_kit::settings;
use crate::store::{ArgStream, SessionStore};
use crate::store::StreamState;
use qaqh_gpui_kit::theme::ActiveTheme;
use qaqh_gpui_kit::theme::Palette;
use qaqh_gpui_kit::primitives::action_button;
use crate::store::{next_model, next_option, model_effort_levels};

pub(crate) fn content_text(value: &ContentValue) -> &str {
    match value {
        ContentValue::Inline { text } => text,
        ContentValue::Ref { .. } => "引用内容：点击上方「加载引用内容」读取",
        ContentValue::Unavailable(_) => "内容不可用",
    }
}

fn state_label(state: &ToolCallState) -> &'static str {
    match state {
        ToolCallState::Declared => "已声明",
        ToolCallState::AwaitingInteraction { .. } => "等待授权",
        ToolCallState::Running => "执行中",
        ToolCallState::Cancelling => "取消中",
        ToolCallState::Finished { outcome } => match outcome {
            ToolCallOutcome::Completed => "完成",
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

fn activity_label(activity: ActivityState) -> &'static str {
    match activity {
        ActivityState::Idle => "空闲",
        ActivityState::Running => "运行中",
        ActivityState::WaitingUser => "等待输入",
        ActivityState::Cancelling => "取消中",
    }
}

/// 侧栏状态点:running/waiting_user/cancelling 显示,其余不显示。
pub fn status_dot(status: &str, t: &Palette) -> Div {
    let (color, label) = match status {
        "running" => (t.accent, "运行中"),
        "waiting_user" => (t.warn, "待响应"),
        "cancelling" => (t.warn, "取消中"),
        "idle" => (t.dim, "空闲"),
        _ => (t.dim, ""),
    };
    if label.is_empty() {
        return div();
    }
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .size_1p5()
                .rounded_full()
                .bg(color),
        )
        .child(div().text_xs().text_color(color).child(label))
}

fn stream_badge(state: &StreamState, t: &Palette) -> Div {
    let (color, label): (Hsla, SharedString) = match state {
        StreamState::Connecting => (t.warn, "连接中".into()),
        StreamState::Open => (t.ok, "实时".into()),
        StreamState::Reconnecting { retry_ms } => {
            (t.warn, SharedString::from(format!("重连 {retry_ms}ms")))
        }
        StreamState::Closed { reason } => (t.bad, SharedString::from(format!("断开:{reason}"))),
    };
    div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .text_xs()
        .bg(qaqh_gpui_kit::kit::soft(color, 0.12))
        .text_color(color)
        .child(label)
}

fn user_bubble(text: &str, t: &Palette) -> Div {
    div()
        .self_end()
        .max_w(px(640.))
        .px_3()
        .py_2()
        .rounded(px(qaqh_gpui_kit::kit::RADIUS_BUBBLE))
        .bg(t.bubble)
        .text_sm()
        .child(text.to_string())
}

/// UTF-8 安全尾部截断(参照 electron 的 16KB 尾策略,不劈多字节字)。
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
        // 子代理/团队协作面(ADR-0014)。
        "spawn_subagent" => ("正在派生子代理", "已派生子代理"),
        "send_message" => ("正在发送代理消息", "已发送代理消息"),
        "wait_agent" => ("正在等待子代理", "子代理已返回"),
        "list_agents" => ("正在查看代理列表", "已查看代理列表"),
        "interrupt_agent" => ("正在打断子代理", "已打断子代理"),
        // 任务板(electron v3.1a 施工单 §二)。
        "task_create" => ("正在创建任务", "已创建任务"),
        "task_claim" => ("正在认领任务", "已更新认领"),
        "task_update" => ("正在更新任务", "已更新任务"),
        "task_close" => ("正在关闭任务", "已关闭任务"),
        "task_list" => ("正在列出任务", "已列出任务"),
        // 留言板。
        "board_channel_create" => ("正在创建频道", "已创建频道"),
        "board_thread_create" => ("正在创建话题", "已创建话题"),
        "board_post" => ("正在留言", "已留言"),
        "board_subscribe" => ("正在更新订阅", "已更新订阅"),
        "board_list" => ("正在查看留言板", "已查看留言板"),
        // Goal(ADR-0015)。
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

fn tool_status_only(name: &str) -> bool {
    matches!(name, "ask" | "todo_write" | "todo_update" | "todo_list" | "task_list" | "board_list" | "list_agents" | "get_goal")
}

fn tool_status_line(title: String, t: &Palette) -> Div {
    div().px_2().py_1().text_xs().text_color(t.dim).truncate()
        .child(SharedString::from(title))
}

fn tool_toggle(
    entity: &Entity<SessionStore>,
    call_id: &ToolCallId,
    expanded: bool,
    title: String,
    t: &Palette,
) -> gpui::Stateful<Div> {
    let entity = entity.downgrade();
    let call_id = call_id.clone();
    div()
        .id(SharedString::from(format!("tool-toggle-{}", call_id.0)))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .text_color(t.dim)
        .cursor_pointer()
        .hover(|style| style.bg(t.btn_hover))
        .child(div().flex_shrink_0().child(if expanded { "▾" } else { "▸" }))
        .child(div().min_w_0().truncate().child(SharedString::from(title)))
        .on_click(move |_, _, cx| {
            let _ = entity.update(cx, |store, cx| {
                if !store.expanded_tools.remove(&call_id) {
                    store.expanded_tools.insert(call_id.clone());
                }
                cx.notify();
            });
        })
}

/// spawn_subagent 的 fork 徽标后缀:mode:"fork" = 上下文继承,标注「继承历史」。
/// 只认声明后的完整 args;参数未定形时不猜。
fn fork_suffix(name: &str, args: Option<&serde_json::Value>) -> &'static str {
    let is_fork = name == "spawn_subagent"
        && args
            .and_then(|args| args.get("mode"))
            .and_then(|mode| mode.as_str())
            == Some("fork");
    if is_fork { " · 继承历史" } else { "" }
}

fn tool_card(
    cx: &gpui::App,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    tool: &ToolCallView,
    t: &Palette,
    tail_max: usize,
) -> Div {
    let name = tool.tool_name.as_deref().unwrap_or("(未知工具)");
    let exec = store.exec_sessions.iter().find(|exec| exec.origin_call_id == tool.call_id);
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
        format!("{name}  $ {command}  ·  {status}")
    } else if let Some(summary) = tool_summary(name, &tool.state) {
        format!("{summary}{fork_suffix}")
    } else {
        format!("{name}  ·  {status}{fork_suffix}")
    };
    if tool_status_only(name) {
        return tool_status_line(title, t);
    }
    let expanded = store.expanded_tools.contains(&tool.call_id);
    let header = tool_toggle(entity, &tool.call_id, expanded, title, t);
    if !expanded {
        return div().child(header);
    }
    // 展开卡:圆角卡面 + 左侧状态色条,命令行用 code 面层收进"终端"质感。
    // card 持续累积子元素(参数/输出/控制),外层横排只放色条+内容列,
    // 内容列在末尾组装(见 assemble)。
    let content = div().flex().flex_col().gap_1p5().py_2().pr_3().min_w_0()
        .child(header)
        .child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .flex_shrink_0()
                        .text_color(t.accent)
                        .child(SharedString::from(format!("⚙ {name}"))),
                )
                .children(command.map(|command| {
                    div()
                        .min_w_0()
                        .text_sm()
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .bg(t.code_bg)
                        .text_color(t.text)
                        .truncate()
                        .child(SharedString::from(format!("$ {command}")))
                }))
                .child(qaqh_gpui_kit::kit::pill(status, status_color, t)),
        );
    let mut card = content;
    let assemble = |content: Div, status_color: Hsla, t: &Palette| {
        div()
            .flex()
            .max_w_full()
            .rounded(px(qaqh_gpui_kit::kit::RADIUS_CONTROL))
            .border_1()
            .border_color(t.border)
            .bg(t.card)
            .overflow_hidden()
            .child(div().w(px(3.)).flex_shrink_0().bg(status_color))
            .child(content)
    };
    if !is_exec && let Some(args) = &tool.args {
        card = card.child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child(SharedString::from(format!("参数 {args}"))),
        );
    }
    let raw_output = if let Some(exec) = exec {
        store.exec_output_tail.iter()
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
    // 历史快照可能只保留工具结果，提取命令输出而非展示结果 JSON。
    let result = if is_exec && exec.is_none() {
        serde_json::from_str::<serde_json::Value>(raw_output).ok()
    } else {
        None
    };
    let output = result.as_ref()
        .and_then(|result| result.get("output")?.as_str())
        .unwrap_or(raw_output);
    let (output, truncated) = utf8_tail(output, tail_max);
    if !output.is_empty() {
        card = card.child(
            div()
                .text_xs()
                .px_2()
                .py_1p5()
                .rounded(px(5.))
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
            card = card.child(div().text_xs().text_color(t.faint).child("(仅显示输出尾部)"));
        }
    }
    if let Some(exec) = exec && !exec.state.is_terminal() {
        let id = exec.id.0.clone();
        let control = |terminate| {
            let entity = entity.downgrade();
            let exec_id = id.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                let _ = entity.update(cx, |store, _| store.exec_control(exec_id.clone(), terminate));
            }
        };
        card = card.child(
            div()
                .flex()
                .gap_2()
                .child(action_button(cx, format!("exec-int-{id}"), "中断", t.warn, control(false), t))
                .child(action_button(cx, format!("exec-term-{id}"), "终止", t.bad, control(true), t)),
        );
    }
    if let Some((summary, doc)) = handoff_fork_payload(tool) {
        // 封箱成功 → 回合尾部入口(electron HandoffCard 同位):新会话继续交接。
        let entity = entity.downgrade();
        card = card.child(
            div().flex().gap_2().child(action_button(
                cx,
                "handoff-fork".into(),
                "继续这条交接 →",
                t.accent,
                move |_, _, cx: &mut gpui::App| {
                    let _ = entity.update(cx, |store, _| store.fork_handoff(summary.clone(), doc.clone()));
                },
                t,
            )),
        );
    }
    assemble(card, status_color, t)
}

/// handoff 工具的 fork 事实:(work_summary, handoff_doc)。
/// 判据同 electron:tool_name == "handoff" 且封箱成功;文档取 args(模型亲手写的那份)。
/// args 缺文档时不给按钮——不猜 output 里的自由文本。
fn handoff_fork_payload(tool: &ToolCallView) -> Option<(Option<String>, String)> {
    if tool.tool_name.as_deref() != Some("handoff") {
        return None;
    }
    let success = matches!(&tool.state, ToolCallState::Finished { outcome } if outcome.is_success());
    if !success {
        return None;
    }
    let args = tool.args.as_ref()?;
    let doc = args.get("handoff_doc")?.as_str()?.to_string();
    if doc.trim().is_empty() {
        return None;
    }
    let summary = args
        .get("work_summary")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty());
    Some((summary, doc))
}

fn has_assistant_text(kind: AssistantBlockKind, text: &str) -> bool {
    kind != AssistantBlockKind::ToolCall && !text.trim().is_empty()
}

fn hidden_timeline_entry(entry: &TimelineEntry) -> bool {
    match entry {
        TimelineEntry::ToolResult { .. } => true,
        TimelineEntry::AssistantBlock { block_kind, content, .. } => {
            !has_assistant_text(*block_kind, content_text(content))
        }
        _ => false,
    }
}

/// 助手块 markdown 区。流式光标单独渲染，避免改变 Markdown 源。
fn assistant_section(
    kind: &AssistantBlockKind,
    text: &str,
    live: bool,
    section: usize,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    t: &Palette,
    ty: &settings::Typography,
) -> gpui::Stateful<Div> {
    let label = match kind {
        AssistantBlockKind::Reasoning => "思考",
        AssistantBlockKind::Answer => "回答",
        AssistantBlockKind::ToolCall => "工具",
    };
    let blocks = store.md_cache.get(section, text);
    div()
        .id(("mdsec", section))
        .flex()
        .flex_col()
        .gap_1()
        .max_w_full()
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.faint)
                .child(label),
        )
        .children(crate::markdown::render_blocks(&blocks, section, t, ty, entity))
        .children(live.then(|| div().text_xs().text_color(t.dim).child("▍")))
}

/// 通知行:左侧色点 + 标签 + 正文(系统陈述/排队/插话共用,非聊天气泡)。
fn notice_line(label: &str, text: &str, color: Hsla, t: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .px_3()
        .py_1p5()
        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CARD))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(color)
                .child(div().size_1p5().rounded_full().bg(color))
                .child(SharedString::from(label.to_owned())),
        )
        .child(div().text_sm().text_color(t.text).child(SharedString::from(text.to_owned())))
}

/// 子代理终态通知卡:按 terminal 着色的系统卡,正文是 runtime 摘要。
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
        title.push_str(&format!("(调用 {})", terminal.parent_call_id));
    }
    notice_line(&title, text, color, t)
}

/// 封口块的用量注记:tok 数 + 权威产出速率(completion_tokens / generated_ms,
/// 内核实测区间为分母;缺 generated_ms 时不给速率,不用到货时间编造)。
fn usage_note(
    usage: Option<&qaqh_types::UsageInfo>,
    generated_ms: Option<u64>,
    t: &Palette,
) -> Option<Div> {
    let usage = usage?;
    let mut note = format!("{} tok", usage.completion_tokens);
    if let Some(ms) = generated_ms.filter(|ms| *ms >= 500) {
        let rate = usage.completion_tokens as f64 / (ms as f64 / 1000.0);
        note.push_str(&format!(" · {rate:.1} tok/s"));
    }
    Some(
        div()
            .text_xs()
            .text_color(t.faint)
            .child(SharedString::from(note)),
    )
}

fn timeline_entry(
    cx: &gpui::App,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    entry: &TimelineEntry,
    index: usize,
    t: &Palette,
    ty: &settings::Typography,
    tail_max: usize,
) -> Option<AnyElement> {
    Some(match entry {
        TimelineEntry::Input {
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
                // 终态通知是运行时陈述,不是子代理说的话:系统卡,不渲染成气泡。
                terminal_notice(terminal, &text, t).into_any_element()
            } else if actor.kind == ActorKind::System && text.starts_with("<qaqh_goal_continuation") {
                notice_line("目标续跑", &text, t.warn, t).into_any_element()
            } else if let Some(envelope) = inter_agent {
                notice_line(
                    &format!("跨代理消息 · 来自 {}", envelope.author),
                    &text,
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
                notice_line(label, &text, t.dim, t).into_any_element()
            } else {
                let mut row = match purpose {
                    InputPurpose::TriggerTurn => div().child(user_bubble(&text, t)),
                    InputPurpose::QueueOnly => notice_line("排队消息", &text, t.dim, t),
                    InputPurpose::Steer => notice_line("运行中转向", &text, t.warn, t),
                    InputPurpose::Interject => notice_line("运行中插话", &text, t.warn, t),
                };
                row = row
                    .child(div().flex().flex_wrap().gap_2().children(images.iter().map(qaqh_gpui_render::media::preview)))
                    .children((!attachments.is_empty()).then(|| div().text_xs().child(format!("{} 个文本附件", attachments.len()))));
                row.into_any_element()
            }
        }
        TimelineEntry::AssistantBlock { block_kind, content, usage, generated_ms, .. } => {
            let section = assistant_section(block_kind, content_text(content), false, index * 2, store, entity, t, ty);
            div()
                .flex()
                .flex_col()
                .max_w_full()
                .child(section)
                .children(usage_note(usage.as_ref(), *generated_ms, t))
                .into_any_element()
        }
        TimelineEntry::ToolCall { call_id, .. } => {
            let tool = store.tools.iter().find(|tool| &tool.call_id == call_id)?;
            tool_card(cx, store, entity, tool, t, tail_max).into_any_element()
        }
        TimelineEntry::Compaction { .. } => div()
            .text_xs()
            .text_color(t.dim)
            .child("── 已压缩 ──")
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
    // 待交互卡:warn 色条 + 软底,和工具卡同一"色条卡"语言。
    div()
        .flex()
        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CARD))
        .border_1()
        .border_color(t.border)
        .bg(t.warn_bg)
        .overflow_hidden()
        .child(div().w(px(3.)).flex_shrink_0().bg(t.warn))
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .py_2p5()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.warn)
                        .child(SharedString::from(format!("需要确认 · {kind_label}"))),
                )
                .child(div().text_sm().child(SharedString::from(request.to_string())))
                .when(!body.is_empty(), |card| {
                    card.child(div().flex().flex_col().gap_1().children(body))
                })
                .when(!buttons.is_empty(), |card| {
                    card.child(div().flex().gap_2().children(buttons))
                }),
        )
}

impl SessionStore {
    /// ask 应答 Dialog 内容:解析 request 渲染问题/选项/自定义输入;
    /// 解析失败回退自由文本。提交/跳过成功后收口对话框。
    fn ask_dialog(
        &mut self,
        interaction: &qaqh_ringing_contract::state::InteractionView,
        dialog: Dialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Dialog {
        let t = *cx.colors();
        let iid = interaction.interaction_id.clone();
        let questions = ask::parse_request(&interaction.request);
        match questions {
            Ok(questions) if !questions.is_empty() => {
                let ids: Vec<String> = questions.iter().map(|q| q.id.clone()).collect();
                let mut body: Vec<AnyElement> = Vec::new();
                if let Some(hint) = &self.ask_hint {
                    body.push(
                        div()
                            .text_xs()
                            .text_color(t.bad)
                            .child(SharedString::from(hint.clone()))
                            .into_any_element(),
                    );
                }
                for (index, question) in questions.iter().enumerate() {
                    let field = self.ask_draft(&iid, questions.len(), window, cx).customs[index].clone();
                    let mut row = div().flex().flex_col().gap_1();
                    row = row.child(
                        div()
                            .text_sm()
                            .child(SharedString::from(format!(
                                "{}. {}",
                                index + 1,
                                question.question
                            ))),
                    );
                    for option in &question.options {
                        let chosen = self.ask_draft(&iid, questions.len(), window, cx).answers[index]
                            .as_deref()
                            == Some(option.as_str());
                        let opt = option.clone();
                        let opt_click = opt.clone();
                        let iid_click = iid.clone();
                        row = row.child(
                            div()
                                .id(SharedString::from(format!("ask-{iid}-{index}-{opt}")))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .border_1()
                                .border_color(if chosen { t.accent } else { t.border })
                                .bg(if chosen { t.accent_bg } else { t.btn })
                                .text_color(if chosen { t.accent } else { t.text })
                                .hover(|style| style.bg(t.btn_hover))
                                .on_click(cx.listener(
                                    move |store: &mut SessionStore, _, window: &mut Window, cx| {
                                        let draft = store.ask_draft(&iid_click, index + 1, window, cx);
                                        draft.answers[index] = Some(opt_click.clone());
                                        draft.customs[index].update(cx, |custom, cx| custom.set_value("", window, cx));
                                        cx.notify();
                                    },
                                ))
                                .child(SharedString::from(format!(
                                    "{} {opt}",
                                    if chosen { "●" } else { "○" }
                                ))),
                        );
                    }
                    if question.allow_custom {
                        row = row.child(Input::new(&field).small().w_full());
                    }
                    body.push(row.into_any_element());
                }
                let submit = {
                    let iid_submit = iid.clone();
                    let ids = ids.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        if store.ask_submit(&iid_submit, &ids, cx).is_some() {
                            store.ask_hint = None;
                            window.close_dialog(cx);
                        } else {
                            store.ask_hint = Some("请先选择答案或填写自定义回答".into());
                        }
                        cx.notify();
                    })
                };
                let skip = {
                    let iid_skip = iid.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        store.ask_dismiss(&iid_skip);
                        store.ask_hint = None;
                        window.close_dialog(cx);
                        cx.notify();
                    })
                };
                dialog
                    .title("会话提问")
                    .children(body)
                    .footer(
                        div().flex().justify_end().gap_2()
                            .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                                cx,
                                qaqh_gpui_kit::kit::BtnKind::Soft,
                                format!("ask-submit-{iid}"),
                                "提交回答",
                                &t,
                                submit,
                            )))
                            .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                                cx,
                                qaqh_gpui_kit::kit::BtnKind::Ghost,
                                format!("ask-skip-{iid}"),
                                "跳过",
                                &t,
                                skip,
                            ))),
                    )
            }
            _ => {
                // 解析失败:回退自由文本(q1),不猜语法。
                let field = self.ask_draft(&iid, 1, window, cx).customs[0].clone();
                let submit = {
                    let ids = vec!["q1".to_string()];
                    let iid_submit = iid.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        if store.ask_submit(&iid_submit, &ids, cx).is_some() {
                            store.ask_hint = None;
                            window.close_dialog(cx);
                        } else {
                            store.ask_hint = Some("请填写回答".into());
                        }
                        cx.notify();
                    })
                };
                let skip = {
                    let iid_skip = iid.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        store.ask_dismiss(&iid_skip);
                        store.ask_hint = None;
                        window.close_dialog(cx);
                        cx.notify();
                    })
                };
                let mut body: Vec<AnyElement> = Vec::new();
                body.push(
                    div()
                        .text_sm()
                        .child(SharedString::from(content_text(&interaction.request).to_owned()))
                        .into_any_element(),
                );
                if let Some(hint) = &self.ask_hint {
                    body.push(
                        div()
                            .text_xs()
                            .text_color(t.bad)
                            .child(SharedString::from(hint.clone()))
                            .into_any_element(),
                    );
                }
                body.push(Input::new(&field).small().w_full().into_any_element());
                dialog
                    .title("会话提问")
                    .children(body)
                    .footer(
                        div().flex().justify_end().gap_2()
                            .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                                cx,
                                qaqh_gpui_kit::kit::BtnKind::Soft,
                                format!("ask-submit-{iid}"),
                                "提交回答",
                                &t,
                                submit,
                            )))
                            .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                                cx,
                                qaqh_gpui_kit::kit::BtnKind::Ghost,
                                format!("ask-skip-{iid}"),
                                "跳过",
                                &t,
                                skip,
                            ))),
                    )
            }
        }
    }
}

/// 打开 ask 应答 Dialog(模态覆盖层,Root 承载):到达即弹、footer 可重开;
/// 提问已处理时显示收口态。
pub fn open_ask_dialog(
    store: &Entity<SessionStore>,
    interaction_id: qaqh_ringing_contract::fact::InteractionId,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    window.open_dialog(cx, {
        let store = store.clone();
        move |dialog, window, cx| {
            store.update(cx, |store, cx| {
                let t = *cx.colors();
                match store
                    .interactions
                    .iter()
                    .find(|view| view.interaction_id == interaction_id)
                {
                    Some(interaction) if interaction.resolution.is_none() => {
                        let interaction = interaction.clone();
                        store.ask_dialog(&interaction, dialog, window, cx)
                    }
                    _ => dialog
                        .title("会话提问")
                        .child(div().text_sm().text_color(t.dim).child("该提问已处理。"))
                        .footer(
                            div().flex().justify_end().child(action_button(
                                cx,
                                "ask-dialog-done".into(),
                                "关闭",
                                t.dim,
                                |_, window: &mut Window, cx: &mut gpui::App| window.close_dialog(cx),
                                &t,
                            )),
                        ),
                }
            })
        }
    });
}

impl Render for SessionStore {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.colors();
        let title = self.title.clone().unwrap_or_else(|| self.session_id.clone());

        // 头部:标题(或重命名输入)+ 流徽标 + 重命名/中断按钮。
        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(t.border)
            .bg(t.panel);
        if let Some(draft) = self.renaming.clone() {
            // Enter 提交由 begin_rename 的事件订阅驱动;Escape 关闭重命名
            // (在祖先层截获,先于输入引擎的 Escape 动作)。
            let rename_input = div()
                .id("rename-input")
                .min_w(px(200.))
                .on_key_down(cx.listener(
                    |store: &mut SessionStore, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            store.rename_cancel();
                            cx.stop_propagation();
                            cx.notify();
                        }
                    },
                ))
                .child(Input::new(&draft).small());
            header = header
                .child(rename_input)
                .child(
                    action_button(cx, 
                        "rename-commit".into(),
                        "✓",
                        t.ok,
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.rename_commit(cx);
                            cx.notify();
                        }),
                        &t,
                    ),
                )
                .child(
                    action_button(cx, 
                        "rename-cancel".into(),
                        "✕",
                        t.dim,
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.rename_cancel();
                            cx.notify();
                        }),
                        &t,
                    ),
                );
        } else {
            header = header
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(stream_badge(&self.stream, &t))
                .children(self.goal.clone().map(|goal| {
                    crate::goal::goal_badge(&goal, cx, &t).into_any_element()
                }))
                .children(self.handoff_seal.as_ref().map(|seal| {
                    // ADR-0016 投影:标记旧线已封箱;点击经 Select 跳后继会话。
                    qaqh_gpui_kit::kit::pill(&format!("已封箱 → {}", seal.work_summary), t.dim, &t)
                        .id("handoff-seal")
                        .cursor_pointer()
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.open_handoff_successor();
                            cx.notify();
                        }))
                        .into_any_element()
                }))
                .children((self.activity == Some(ActivityState::Running))
                    .then(|| self.tok_rate())
                    .flatten()
                    .map(|rate| {
                        qaqh_gpui_kit::kit::pill(&format!("≈ {:.0} tok/s", rate.max(1.)), t.accent, &t)
                            .into_any_element()
                    }))
                .child(
                    div()
                        .id("model-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CONTROL))
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            let current = store.settings.as_ref().and_then(|s| s.meta.model_selection.as_ref());
                            if let Some(catalog) = &store.catalog {
                                let options = crate::store::model_options_from(catalog);
                                if let Some(next) = next_model(&options, current).cloned() {
                                    store.patch_model(next);
                                    cx.notify();
                                }
                            }
                        }))
                        .child(format!(
                            "模型:{}",
                            self.settings.as_ref().and_then(|s| s.meta.model_selection.as_ref())
                                .map(|model| format!("{}/{}", model.provider_id, model.model_id))
                                .unwrap_or_else(|| "未选择".into()),
                        )),
                )
                .child(
                    div()
                        .id("effort-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CONTROL))
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            let model = store.settings.as_ref().and_then(|settings| settings.meta.model_selection.as_ref());
                            if let Some((catalog, model)) = store.catalog.as_deref().zip(model) {
                                let levels = model_effort_levels(catalog, model);
                                let current = model.options.reasoning_effort.as_ref();
                                let next = match current {
                                    Some(current) => levels.iter().position(|level| level == current)
                                        .and_then(|index| levels.get(index + 1)),
                                    None => levels.first(),
                                };
                                store.patch_effort(next.cloned());
                                cx.notify();
                            }
                        }))
                        .child(format!("强度:{}", self.settings.as_ref()
                            .and_then(|s| s.meta.model_selection.as_ref())
                            .and_then(|model| model.options.reasoning_effort.as_deref())
                            .unwrap_or("默认"))),
                )
                .child(
                    div()
                        .id("tools-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CONTROL))
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            let current = store.settings.as_ref().map(|s| s.meta.tool_mode.as_str());
                            let presets = [qaqh_types::STANDARD, qaqh_types::MINIMAL, qaqh_types::MINIMAL_B, qaqh_types::MINIMAL_C];
                            if let Some(next) = next_option(&presets, current.as_ref()) {
                                store.patch_tools_mode((*next).to_string());
                                cx.notify();
                            }
                        }))
                        .child(format!("工具:{}", self.settings.as_ref()
                            .map(|s| s.meta.tool_mode.as_str()).filter(|mode| !mode.is_empty())
                            .unwrap_or(qaqh_types::STANDARD))),
                )
                .child(
                    div()
                        .id("detach-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CONTROL))
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.detach_from_workspace();
                            cx.notify();
                        }))
                        .child("脱离"),
                )
                .child(
                    action_button(cx, 
                        format!("rename-{}", self.session_id),
                        "重命名",
                        t.dim,
                        cx.listener(|store: &mut SessionStore, _, window, cx| {
                            store.begin_rename(window, cx);
                            cx.notify();
                        }),
                        &t,
                    ),
                )
                .child(
                    gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                        cx,
                        qaqh_gpui_kit::kit::BtnKind::Danger,
                        format!("delete-{}", self.session_id),
                        if self.confirm_delete { "确认删除" } else { "删除" },
                        &t,
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            if store.confirm_delete {
                                store.delete_self();
                            }
                            store.confirm_delete = !store.confirm_delete;
                            cx.notify();
                        }),
                    )),
                );
        }
        if self.activity == Some(ActivityState::Running) {
            header = header.child(
                div().flex_1().child(div()).child(
                    action_button(cx, 
                        format!("cancel-{}", self.session_id),
                        "中断",
                        t.bad,
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.cancel_turn();
                            // 看门狗:5s 无终态则本地收口 + 重灌快照。
                            store.arm_cancel_watchdog(5000);
                            cx.notify();
                        }),
                        &t,
                    ),
                ),
            );
        } else if let Some(activity) = self.activity {
            header = header.child(
                div()
                    .flex_1()
                    .child(div())
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(activity_label(activity)),
                    ),
            );
        }

        let settings = settings::get(cx).clone();
        let ty = settings.typography();
        let tail_max = settings.behavior.output_tail_max;
        self.output_budget = tail_max;

        let list_state = self.timeline_list.clone();
        let old_history_len = list_state.item_count() - 1;
        if old_history_len != self.entries.len() {
            let start = old_history_len.min(self.entries.len());
            list_state.splice(start..old_history_len, self.entries.len() - start);
        }
        let follow = settings.behavior.stream_follow;
        if self.timeline_follow_setting != Some(follow) {
            list_state.set_follow_mode(if follow { FollowMode::Tail } else { FollowMode::Normal });
            self.timeline_follow_setting = Some(follow);
        }
        let entity = cx.entity();
        // 行 padding 挂在行上而非 list 上:gpui list 的滚动范围只从 items 高度
        // 算,list 自带 padding 会让滚动条 thumb 永远到不了底(gpui-component
        // MessageScroller 注释同口径)。
        let timeline = list(list_state, move |index, _, cx| {
            let store = entity.read(cx);
            let row = div().w_full().px_3().when(index == 0, |row| row.pt_3());
            let gap = px((8.0 + ty.density_shift).max(2.));
            if let Some(entry) = store.entries.get(index) {
                if hidden_timeline_entry(entry) {
                    return row.into_any_element();
                }
                let adjacent_tools = matches!(entry, TimelineEntry::ToolCall { .. })
                    && matches!(
                        store.entries.iter().skip(index + 1)
                            .find(|entry| !hidden_timeline_entry(entry)),
                        Some(TimelineEntry::ToolCall { .. })
                    );
                return match timeline_entry(cx, store, &entity, entry, index, &t, &ty, tail_max) {
                    Some(content) => row.pb(if adjacent_tools { px(0.) } else { gap })
                        .child(content).into_any_element(),
                    None => row.into_any_element(),
                };
            }

            let referenced_tools: HashSet<_> = store.entries.iter().filter_map(|entry| {
                match entry {
                    TimelineEntry::ToolCall { call_id, .. } => Some(call_id),
                    _ => None,
                }
            }).collect();
            let footer = row.pb_3().flex().flex_col().gap(gap)
                .children(
                    store.streaming.iter().enumerate()
                        .filter(|(_, (_, kind, text))| has_assistant_text(*kind, text))
                        .map(|(i, (_, kind, text))| {
                            assistant_section(kind, text, true, i * 2 + 1, store, &entity, &t, &ty)
                                .into_any_element()
                        }),
                )
                .children(store.arg_stream.iter().map(|stream| {
                    let ArgStream { call_id, tool_name, text, lines, target_files, args_complete } = stream;
                    if tool_status_only(tool_name) {
                        let title = tool_summary(tool_name, &ToolCallState::Declared)
                            .map_or_else(|| format!("正在准备 {tool_name}"), str::to_owned);
                        return tool_status_line(title, &t).into_any_element();
                    }
                    // 流式参数行:标题带进度注记(行数估算/目标文件/参数完整),
                    // 与 electron 的 ToolArgumentsDelta 注记同口径。
                    let mut title = format!("正在准备 {tool_name}");
                    if let Some((added, removed)) = lines {
                        title.push_str(&format!("  +{added} −{removed}"));
                    }
                    if !target_files.is_empty() {
                        title.push_str(&format!("  · {}", target_files.join(", ")));
                    }
                    if *args_complete {
                        title.push_str("  · 参数已完整");
                    }
                    let expanded = store.expanded_tools.contains(call_id);
                    div()
                        .child(tool_toggle(&entity, call_id, expanded, title, &t))
                        .children(expanded.then(|| {
                            div().px_2().text_xs().text_color(t.dim)
                                .child(SharedString::from(text.clone()))
                        }))
                        .into_any_element()
                }))
                .children(store.tools.iter().filter_map(|tool| {
                    if referenced_tools.contains(&tool.call_id) {
                        return None;
                    }
                    Some(tool_card(cx, store, &entity, tool, &t, tail_max).into_any_element())
                }))
                .children(store.subagents.iter().map(|subagent| {
                    let status = subagent
                        .status
                        .as_ref()
                        .map(|status| format!("{status:?}"))
                        .unwrap_or_else(|| "进行中".into());
                    let mut row = div()
                        .text_xs()
                        .text_color(t.dim)
                        .child(SharedString::from(format!(
                            "🤖 子代理 {} · {status}",
                            subagent.child_session_id.0,
                        )));
                    if let Some(result) = &subagent.result {
                        let head: String = content_text(result).chars().take(160).collect();
                        row = row.child(div().text_xs().text_color(t.dim).child(head));
                    }
                    row.into_any_element()
                }));
            footer.into_any_element()
        })
        .flex_1()
        .min_h_0()
        .w_full()
        .max_w(px(ty.content_width + 24.))
        .mx_auto();

        // 消息浮面(Codex MainContentSurface 口径):圆角 16 的浮层托住时间线,
        // 与四周留 8px 呼吸边;面板色比 bg 提一级,不再通铺到底。
        // 必须显式 .flex().flex_col():gpui div 默认 display 是 Block,
        // Block 流里 list 的 flex_1 失效,高度塌缩为 0 → 正文全空。
        // vertical_scrollbar:滚动条叠在 viewport 右缘,拖拽/滚轮共用 ListState。
        let timeline = div()
            .id("timeline-panel")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .mx_2()
            .mt_2()
            .rounded(px(16.))
            .bg(t.panel)
            .border_1()
            .border_color(t.border)
            .overflow_hidden()
            .vertical_scrollbar(&self.timeline_list)
            .child(timeline);

        // composer:组件库 Textarea(auto_grow,Enter 提交);受阻原因降为
        // 输入框下方常驻提示行(占位符固定为输入引导)。
        let blocked = self.send_block_reason();
        let composer = div()
            .mx_3()
            .mb_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(Textarea::new(&self.composer))
            .children(blocked.map(|reason| {
                div().text_xs().text_color(t.warn).child(SharedString::from(reason))
            }));

        let mut footer = div()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(t.border)
            .bg(t.panel)
            .px_3()
            .py_2();
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
                                store.respond_permission(call.clone(), true, false);
                                cx.notify();
                            },
                        )
                    };
                    let trust = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(call.clone(), true, true);
                                cx.notify();
                            },
                        )
                    };
                    let deny = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(call.clone(), false, false);
                                cx.notify();
                            },
                        )
                    };
                    let uid = interaction.interaction_id.0.clone();
                    footer = footer.child(interaction_card(
                        "工具权限",
                        content_text(&interaction.request),
                        vec![
                            action_button(cx, format!("{uid}-allow"), "允许一次", t.ok, allow, &t)
                                .into_any_element(),
                            action_button(cx, format!("{uid}-trust"), "始终允许", t.ok, trust, &t)
                                .into_any_element(),
                            action_button(cx, format!("{uid}-deny"), "拒绝", t.bad, deny, &t)
                                .into_any_element(),
                        ],
                        Vec::new(),
                        &t,
                    ));
                }
                InteractionKind::Ask => {
                    // 应答内容在模态 Dialog 里;footer 只留重开入口。
                    let entity = cx.entity();
                    let iid = interaction.interaction_id.clone();
                    footer = footer.child(
                        action_button(cx,
                            format!("ask-open-{}", iid.0),
                            "回答提问",
                            t.warn,
                            move |_, window: &mut Window, cx: &mut gpui::App| {
                                open_ask_dialog(&entity, iid.clone(), window, cx);
                            },
                            &t,
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        let conversation = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(crate::controls::session_controls(self, cx, &t))
            .children(crate::controls::search(self, cx))
            .child(timeline)
            .children(crate::code_panel::review_button(self, &t, cx))
            .child(composer)
            .child(crate::controls::composer_tools(self, cx, &t))
            .child(footer);
        div().flex().flex_1().min_w_0()
            .child(conversation)
            .children(crate::code_panel::render(self, &t, &ty, cx))
    }
}