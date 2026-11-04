//! 元素构建(M2):暗色主题 + 侧栏/时间线/composer/交互应答按钮。
//! `Entity<T: Render>` 即元素:DaemonStore 为根,SessionStore 经实体直接挂载。

use gpui::prelude::*;
use gpui::AnyElement;
use gpui::ClickEvent;
use gpui::Context;
use gpui::Div;
use gpui::FontWeight;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::KeyDownEvent;
use gpui::ParentElement;
use gpui::Render;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::div;
use gpui::px;
use gpui::rgb;
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::ContentValue;
use qaqh_ringing_contract::fact::InteractionKind;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallState;
use qaqh_ringing_contract::state::ToolCallView;

use crate::store::DaemonStore;
use crate::store::SessionStore;
use crate::store::SessionRow;
use crate::store::StreamState;

const BG: u32 = 0x111419;
const PANEL: u32 = 0x171b22;
const PANEL_LINE: u32 = 0x242a34;
const TEXT: u32 = 0xd8dee9;
const DIM: u32 = 0x8a93a2;
const ACCENT: u32 = 0x4f9cf9;
const USER_BUBBLE: u32 = 0x1e2a3d;
const OK: u32 = 0x4fbf7a;
const WARN: u32 = 0xd9a03f;
const BAD: u32 = 0xe06c75;

fn content_text(value: &ContentValue) -> &str {
    match value {
        ContentValue::Inline { text } => text,
        _ => "(引用内容,按需下载)",
    }
}

fn state_label(state: &ToolCallState) -> &'static str {
    match state {
        ToolCallState::Declared => "已声明",
        ToolCallState::AwaitingInteraction { .. } => "等待授权",
        ToolCallState::Running => "执行中",
        ToolCallState::Cancelling => "取消中",
        ToolCallState::Finished { .. } => "完成",
    }
}

fn state_color(state: &ToolCallState) -> u32 {
    match state {
        ToolCallState::Declared => DIM,
        ToolCallState::AwaitingInteraction { .. } => WARN,
        ToolCallState::Running => ACCENT,
        ToolCallState::Cancelling => WARN,
        ToolCallState::Finished { .. } => OK,
    }
}

fn stream_badge(state: &StreamState) -> Div {
    let (color, label): (u32, SharedString) = match state {
        StreamState::Connecting => (WARN, "连接中".into()),
        StreamState::Open => (OK, "实时".into()),
        StreamState::Reconnecting { retry_ms } => {
            (WARN, SharedString::from(format!("重连 {retry_ms}ms")))
        }
        StreamState::Closed { reason } => (BAD, SharedString::from(format!("断开:{reason}"))),
    };
    div()
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_xs()
        .bg(rgb(0x1c2129))
        .text_color(rgb(color))
        .child(label)
}

fn sidebar_row(
    row: &SessionRow,
    selected: bool,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let title = if row.title.is_empty() {
        "(无标题)".to_string()
    } else {
        row.title.clone()
    };
    div()
        .id(SharedString::from(format!("sess-{}", row.id)))
        .px_3()
        .py_2()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x1e242e)))
        .when(selected, |style| style.bg(rgb(0x232c3a)))
        .on_click(listener)
        .flex()
        .flex_col()
        .gap_0p5()
        .child(
            div()
                .text_sm()
                .text_color(rgb(if selected { ACCENT } else { TEXT }))
                .child(title),
        )
        .child(div().text_xs().text_color(rgb(DIM)).child(row.updated_at.clone()))
}

impl Render for DaemonStore {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<AnyElement> = self
            .sessions
            .iter()
            .map(|row| {
                let selected = self
                    .current
                    .as_ref()
                    .is_some_and(|current| current.read(cx).session_id == row.id);
                let id = row.id.clone();
                let listener = cx.listener(move |daemon: &mut DaemonStore, _event, _window, cx| {
                    daemon.select(id.clone(), cx);
                });
                sidebar_row(row, selected, listener).into_any_element()
            })
            .collect();

        let sidebar = div()
            .w(px(260.))
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(PANEL))
            .border_r_1()
            .border_color(rgb(PANEL_LINE))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(format!("会话({})", self.sessions.len())),
            )
            .child(
                div()
                    .id("session-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .children(rows),
            )
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(rgb(PANEL_LINE))
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(self.lines.last().cloned().unwrap_or_default()),
            );

        let main = match self.current.clone() {
            Some(session) => div().flex_1().min_w_0().child(session),
            None => div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(DIM))
                .child("正在连接 daemon…"),
        };

        div()
            .size_full()
            .flex()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .child(sidebar)
            .child(main)
    }
}

fn user_bubble(text: &str) -> Div {
    div()
        .self_end()
        .max_w(px(640.))
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(rgb(USER_BUBBLE))
        .text_sm()
        .child(text.to_string())
}

fn assistant_text(kind: &AssistantBlockKind, text: &str, live: bool) -> Div {
    let label = match kind {
        AssistantBlockKind::Reasoning => "思考",
        AssistantBlockKind::Answer => "回答",
        AssistantBlockKind::ToolCall => "工具",
    };
    let shown = if live {
        format!("{text}▍")
    } else {
        text.to_string()
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .max_w_full()
        .child(div().text_xs().text_color(rgb(DIM)).child(label))
        .child(div().text_sm().child(shown))
}

fn tool_card(tool: &ToolCallView, tail: Option<&String>) -> Div {
    let name = tool.tool_name.as_deref().unwrap_or("(未知工具)");
    let mut card = div()
        .flex()
        .flex_col()
        .gap_1()
        .max_w_full()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(PANEL_LINE))
        .bg(rgb(PANEL))
        .child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .text_color(rgb(ACCENT))
                        .child(SharedString::from(format!("⚙ {name}"))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(rgb(state_color(&tool.state)))
                        .child(state_label(&tool.state)),
                ),
        );
    if let Some(args) = &tool.args {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(DIM))
                .child(SharedString::from(format!("参数 {args}"))),
        );
    }
    let output: Option<String> = match &tool.state {
        ToolCallState::Finished { .. } => tool
            .output
            .as_ref()
            .map(content_text)
            .map(|text| text.to_string()),
        _ => tail.cloned(),
    };
    if let Some(output) = output
        && !output.is_empty()
    {
        card = card.child(
            div()
                .text_xs()
                .text_color(rgb(DIM))
                .child(SharedString::from(output)),
        );
    }
    card
}

fn timeline_entry(entry: &TimelineEntry, store: &SessionStore) -> Option<AnyElement> {
    match entry {
        TimelineEntry::Input { content, .. } => {
            Some(user_bubble(content_text(content)).into_any_element())
        }
        TimelineEntry::AssistantBlock {
            block_kind,
            content,
            ..
        } => Some(assistant_text(block_kind, content_text(content), false).into_any_element()),
        TimelineEntry::ToolCall { call_id, .. } => {
            let tool = store.tools.iter().find(|tool| &tool.call_id == call_id)?;
            let tail = store
                .output_tail
                .iter()
                .find(|slot| &slot.0 == call_id)
                .map(|slot| &slot.1);
            Some(tool_card(tool, tail).into_any_element())
        }
        TimelineEntry::ToolResult { .. } => None,
        TimelineEntry::Compaction { .. } => Some(
            div()
                .text_xs()
                .text_color(rgb(DIM))
                .child("── 已压缩 ──")
                .into_any_element(),
        ),
    }
}

fn action_button(
    id: String,
    label: &'static str,
    color: u32,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .cursor_pointer()
        .bg(rgb(0x1f2630))
        .text_color(rgb(color))
        .hover(|style| style.bg(rgb(0x27303d)))
        .on_click(listener)
        .child(label)
}

fn interaction_card(
    uid: String,
    kind_label: &str,
    request: &str,
    choices_note: Option<String>,
    buttons: Vec<AnyElement>,
) -> Div {
    let mut card = div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(rgb(WARN))
        .bg(rgb(0x241f14));
    card = card.child(
        div()
            .text_sm()
            .text_color(rgb(WARN))
            .child(SharedString::from(format!("待交互 · {kind_label}"))),
    );
    card = card.child(div().text_sm().child(SharedString::from(request.to_string())));
    if let Some(note) = choices_note {
        card = card.child(div().text_xs().text_color(rgb(DIM)).child(note));
    }
    if !buttons.is_empty() {
        card = card.child(div().flex().gap_2().children(buttons));
    }
    card
}

impl SessionStore {
    fn composer_key(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = &keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform || modifiers.function {
            return;
        }
        match keystroke.key.as_str() {
            "enter" => {
                if modifiers.shift {
                    self.composer_push("\n");
                } else {
                    self.composer_send();
                }
                cx.notify();
            }
            "backspace" => {
                self.composer_backspace();
                cx.notify();
            }
            "space" => {
                self.composer_push(" ");
                cx.notify();
            }
            "tab" | "escape" | "up" | "down" | "left" | "right" | "home" | "end"
            | "pageup" | "pagedown" | "delete" => {}
            key => {
                if let Some(ime) = &keystroke.ime {
                    let owned = ime.clone();
                    self.composer_push(&owned);
                    cx.notify();
                } else if key.len() == 1 {
                    self.composer_push(key);
                    cx.notify();
                }
            }
        }
    }
}

impl Render for SessionStore {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title.clone().unwrap_or_else(|| self.session_id.clone());
        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(rgb(PANEL_LINE))
            .bg(rgb(PANEL))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(stream_badge(&self.stream));
        if self.activity == Some(ActivityState::Running) {
            let cancel = cx.listener(move |store: &mut SessionStore, _, _, cx| {
                store.cancel_turn();
                cx.notify();
            });
            header = header.child(
                div().flex_1().child(div()).child(
                    action_button(
                        format!("cancel-{}", self.session_id),
                        "中断",
                        BAD,
                        cancel,
                    ),
                ),
            );
        }

        let timeline = div()
            .id("timeline")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .children(
                self.entries
                    .iter()
                    .filter_map(|entry| timeline_entry(entry, self)),
            )
            .children(self.streaming.iter().map(|(_, kind, text)| {
                assistant_text(kind, text, true).into_any_element()
            }))
            .children(self.arg_stream.iter().map(|(_, name, text)| {
                div()
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(SharedString::from(format!(
                        "正在生成 {name} 参数… {text}"
                    )))
                    .into_any_element()
            }))
            .children(self.tools.iter().filter_map(|tool| {
                let referenced = self.entries.iter().any(|entry| {
                    matches!(entry, TimelineEntry::ToolCall { call_id, .. } if call_id == &tool.call_id)
                });
                if referenced {
                    return None;
                }
                let tail = self
                    .output_tail
                    .iter()
                    .find(|slot| slot.0 == tool.call_id)
                    .map(|slot| &slot.1);
                Some(tool_card(tool, tail).into_any_element())
            }))
            .children(self.exec_sessions.iter().map(|exec| {
                div()
                    .text_xs()
                    .text_color(rgb(DIM))
                    .child(SharedString::from(format!(
                        "$ {}({:?})",
                        exec.command, exec.state
                    )))
                    .into_any_element()
            }));

        let focused = self.composer_focus.is_focused(window);
        let composer = div()
            .id("composer")
            .mx_3()
            .mb_2()
            .px_3()
            .py_2()
            .rounded_md()
            .min_h(px(38.))
            .border_1()
            .border_color(rgb(if focused { ACCENT } else { PANEL_LINE }))
            .bg(rgb(PANEL))
            .cursor_pointer()
            .key_context("Composer")
            .track_focus(&self.composer_focus)
            .on_click(cx.listener(
                |store: &mut SessionStore, _, window: &mut Window, _| {
                    window.focus(&store.composer_focus);
                },
            ))
            .on_key_down(cx.listener(Self::composer_key))
            .child(if self.composer.is_empty() {
                div().text_sm().text_color(rgb(DIM)).child("输入消息,Enter 发送…")
            } else {
                div().text_sm().child(self.composer.clone())
            });

        let mut footer = div()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(rgb(PANEL_LINE))
            .bg(rgb(PANEL))
            .px_3()
            .py_2();
        for interaction in &self.interactions {
            if interaction.resolution.is_some() {
                continue;
            }
            let uid = interaction.interaction_id.0.clone();
            let buttons: Vec<AnyElement> = match interaction.kind {
                InteractionKind::Permission => {
                    let Some(call_id) = &interaction.call_id else {
                        continue;
                    };
                    let call = call_id.0.clone();
                    let allow = cx.listener(
                        move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                            store.respond_permission(call.clone(), true);
                            cx.notify();
                        },
                    );
                    let call = call_id.0.clone();
                    let deny = cx.listener(
                        move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                            store.respond_permission(call.clone(), false);
                            cx.notify();
                        },
                    );
                    vec![
                        action_button(format!("{uid}-allow"), "允许", OK, allow).into_any_element(),
                        action_button(format!("{uid}-deny"), "拒绝", BAD, deny).into_any_element(),
                    ]
                }
                InteractionKind::Plan => {
                    let iid = interaction.interaction_id.0.clone();
                    let approve = cx.listener(
                        move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                            store.respond_plan(iid.clone(), true);
                            cx.notify();
                        },
                    );
                    let iid = interaction.interaction_id.0.clone();
                    let reject = cx.listener(
                        move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                            store.respond_plan(iid.clone(), false);
                            cx.notify();
                        },
                    );
                    vec![
                        action_button(format!("{uid}-approve"), "批准", OK, approve)
                            .into_any_element(),
                        action_button(format!("{uid}-reject"), "拒绝", BAD, reject)
                            .into_any_element(),
                    ]
                }
                InteractionKind::Ask => Vec::new(),
            };
            let choices_note = match interaction.kind {
                InteractionKind::Ask if !interaction.choices.is_empty() => {
                    let labels: Vec<String> =
                        interaction.choices.iter().map(|c| c.label.clone()).collect();
                    Some(format!("可选:{}", labels.join(" / ")))
                }
                _ => None,
            };
            footer = footer.child(interaction_card(
                uid,
                match interaction.kind {
                    InteractionKind::Permission => "工具权限",
                    InteractionKind::Plan => "计划评审",
                    InteractionKind::Ask => "提问",
                },
                content_text(&interaction.request),
                choices_note,
                buttons,
            ));
        }
        let activity = self
            .activity
            .map(|activity| format!("{activity:?}"))
            .unwrap_or_else(|| "未知".into());
        footer = footer.child(
            div()
                .text_xs()
                .text_color(rgb(DIM))
                .child(SharedString::from(format!("活动状态:{activity}"))),
        );

        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(timeline)
            .child(composer)
            .child(footer)
    }
}
