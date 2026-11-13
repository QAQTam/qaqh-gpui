//! Desktop lifecycle and controls, independent of timeline rendering.
use qaqh_gpui_wire::bridge::{BridgeRequest, ConnectionState};
use qaqh_gpui_kit::theme::Palette;
use crate::{
    store::{DaemonStore, SessionRow, SessionStore},
    ui::action_button,
};
use gpui::{App, Context, Div, FontWeight, KeyDownEvent, PathPromptOptions, SharedString, Window, div, prelude::*, px};
use gpui_component::input::Input;
use gpui_component::Sizable;
use qaqh_client::{CommandTarget, RingingCommand, SessionListEntry};

pub fn notify(cx: &App, session: &str, title: &str) {
    cx.show_system_notification(gpui::SystemNotification {
        tag: format!("qaqh-{session}").into(),
        title: title.to_string().into(),
        body: "打开 QAQHarness 查看会话".into(),
        actions: Vec::new(),
    });
}

pub fn roster_notifications(old: &[SessionRow], entries: &[SessionListEntry], cx: &App) {
    for entry in entries {
        if let Some(previous) = old.iter().find(|row| row.id == entry.meta.session_id) {
            let message = match entry.status {
                qaqh_client::SessionRunStatus::WaitingUser if previous.status != "waiting_user" => {
                    Some("会话正在等待你的响应")
                }
                qaqh_client::SessionRunStatus::Idle
                    if previous.status == "running" || previous.status == "cancelling" =>
                {
                    Some("会话已结束本轮工作")
                }
                _ => None,
            };
            if let Some(message) = message {
                notify(cx, &entry.meta.session_id, message);
            }
        }
    }
}

/// 连接状态:收敛为侧栏底部一张紧凑卡(全应用唯一入口),
/// 状态点文案 + 重连/诊断两个幽灵动作;操作反馈经 last_operation 呈现。
pub fn connection(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let (label, color, detail) = match &daemon.connection {
        ConnectionState::Connecting => ("正在连接 daemon…", t.warn, None),
        ConnectionState::Ready { .. } => ("已连接", t.ok, None),
        ConnectionState::Failed { detail } => ("连接失败", t.bad, Some(detail.clone())),
    };
    div()
        .mx_2()
        .mb_2()
        .p_2()
        .rounded(px(qaqh_gpui_kit::kit::RADIUS_CARD))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .flex()
        .flex_col()
        .gap_1p5()
        .child(qaqh_gpui_kit::kit::pill(label, color, t))
        .children(detail.map(|detail| {
            div().text_xs().text_color(t.bad).child(detail)
        }))
        .child(
            div()
                .flex()
                .gap_1()
                .child(action_button(cx,
                    "reconnect".into(),
                    "重新连接",
                    t.accent,
                    cx.listener(|daemon, _, _, cx| {
                        daemon.connection = ConnectionState::Connecting;
                        let _ = daemon.requests.unbounded_send(BridgeRequest::Reconnect);
                        cx.notify();
                    }),
                    t,
                ))
                .child(action_button(cx,
                    "diagnostics".into(),
                    "复制诊断",
                    t.dim,
                    cx.listener(|_, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                            qaqh_gpui_wire::diagnostics::report(),
                        ));
                    }),
                    t,
                )),
        )
        .children(
            daemon
                .last_operation
                .clone()
                .map(|text| div().text_xs().text_color(t.warn).child(text)),
        )
}

/// 侧栏工具区:搜索 + 过滤/入口 chips。窄/宽微调按钮已砍——布局微调
/// 不属于一级界面,宽度走默认值持久化。
pub fn sidebar_tools(
    daemon: &DaemonStore,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    div()
        .px_2()
        .pb_1()
        .flex()
        .flex_col()
        .gap_2()
        .child(Input::new(&daemon.session_search).small())
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .child(action_button(cx,
                    "archive-filter".into(),
                    if daemon.show_archived {
                        "返回活动会话"
                    } else {
                        "查看归档"
                    },
                    t.dim,
                    cx.listener(|daemon, _, _, cx| {
                        daemon.show_archived = !daemon.show_archived;
                        cx.notify();
                    }),
                    t,
                ))
                .child(action_button(cx,
                    "workspace-settings".into(),
                    "工作区",
                    t.dim,
                    cx.listener(|daemon, _, _, cx| {
                        daemon.settings_open = true;
                        daemon.new_session_open = false;
                        daemon.settings_editor = qaqh_gpui_session::store::SettingsEditor::Workspaces;
                        daemon.settings_form = None;
                        cx.notify();
                    }),
                    t,
                )),
        )
}

/// 欢迎页:居中 hero 卡片(glyph + 标题 + 描述 + 连接 pill + 主/次 CTA),
/// 快捷键键帽与版本号退到卡片下方;连接状态只在侧栏渲染,此处不再重复。
pub fn welcome(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let (status_label, status_color) = match &daemon.connection {
        ConnectionState::Connecting => ("正在连接 daemon…", t.warn),
        ConnectionState::Ready { .. } => ("daemon 已连接", t.ok),
        ConnectionState::Failed { .. } => ("daemon 未连接", t.bad),
    };
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_4()
        .bg(t.bg)
        .child(
            qaqh_gpui_kit::kit::hero(t)
                .child(
                    div()
                        .text_2xl()
                        .text_color(t.accent)
                        .child("◈"),
                )
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text)
                        .child("欢迎使用 QAQHarness"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(t.dim)
                        .text_center()
                        .child("先在设置中添加服务商和模型，再选择工作目录并新建会话。"),
                )
                .child(qaqh_gpui_kit::kit::pill(status_label, status_color, t))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .mt_2()
                        .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                            cx,
                            qaqh_gpui_kit::kit::BtnKind::Primary,
                            "setup-model",
                            "配置服务商与模型",
                            t,
                            cx.listener(|daemon, _, _, cx| {
                                daemon.settings_open = true;
                                daemon.settings_editor = qaqh_gpui_session::store::SettingsEditor::Catalog;
                                let _ = daemon.requests.unbounded_send(BridgeRequest::QueryConfig);
                                let _ = daemon.requests.unbounded_send(BridgeRequest::ListProviders);
                                cx.notify();
                            }),
                        )))
                        .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                            cx,
                            qaqh_gpui_kit::kit::BtnKind::Ghost,
                            "welcome-new-session",
                            "新建会话",
                            t,
                            cx.listener(|daemon, _, _, cx| {
                                daemon.new_session_open = true;
                                daemon.settings_open = false;
                                daemon.settings_form = None;
                                let _ = daemon.requests.unbounded_send(BridgeRequest::ListProviders);
                                cx.notify();
                            }),
                        ))),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_xs()
                .text_color(t.faint)
                .child(qaqh_gpui_kit::kit::kbd("Ctrl+N", t))
                .child("新建")
                .child(div().mx_1().child("·"))
                .child(qaqh_gpui_kit::kit::kbd("Ctrl+K", t))
                .child("搜索会话")
                .child(div().mx_1().child("·"))
                .child(qaqh_gpui_kit::kit::kbd("Ctrl+F", t))
                .child("搜索消息")
                .child(div().mx_1().child("·"))
                .child(qaqh_gpui_kit::kit::kbd("Ctrl+,", t))
                .child("设置"),
        )
        .child(
            div()
                .text_xs()
                .text_color(t.faint)
                .child(SharedString::from(format!("版本 {}", qaqh_gpui_wire::diagnostics::version()))),
        )
}

