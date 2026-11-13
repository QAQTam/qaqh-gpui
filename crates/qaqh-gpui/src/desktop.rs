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
                        daemon.settings_editor = crate::store::SettingsEditor::Workspaces;
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
                                daemon.settings_editor = crate::store::SettingsEditor::Catalog;
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
                .child(SharedString::from(format!("版本 {}", env!("QAQH_VERSION")))),
        )
}

/// 会话控制条:状态 pill + 分组幽灵动作(控制权 | 检索/导出 | 会话管理 | 危险区),
/// 分隔线收口;停止会话是破坏性动作,放最右并落 Danger 色。
pub fn session_controls(store: &SessionStore, cx: &mut Context<SessionStore>, t: &Palette) -> Div {
    let holder = store.driver.as_ref().and_then(|d| d.holder.clone());
    let own = holder.is_some() && holder == store.client_session_id;
    let archived = store.settings.as_ref().is_some_and(|s| s.meta.archived);
    let stale_holder = holder.clone();
    let (control_label, control_color) = match holder {
        None => ("控制权 · 空闲", t.dim),
        Some(_) if own => ("控制权 · 本客户端", t.ok),
        Some(_) => ("控制权 · 其他客户端", t.warn),
    };
    div()
        .px_3()
        .py_1()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_1()
        .text_xs()
        .child(qaqh_gpui_kit::kit::pill(control_label, control_color, t))
        .child(action_button(cx,
            "driver-control".into(),
            if own {
                "释放控制权"
            } else if stale_holder.is_some() {
                "接管控制权"
            } else {
                "取得控制权"
            },
            t.accent,
            cx.listener(move |store, _, _, _| {
                let _ = store.requests().unbounded_send(BridgeRequest::Driver {
                    seed: store.session_id.clone(),
                    release: own,
                    stale_holder: stale_holder.clone(),
                });
            }),
            t,
        ))
        .child(v_divider(t))
        .child(action_button(cx,
            "message-search".into(),
            "搜索消息",
            t.dim,
            cx.listener(|store, _, window, cx| {
                store.search_open = !store.search_open;
                store.search.update(cx, |field, cx| field.focus(window, cx));
                cx.notify();
            }),
            t,
        ))
        .child(action_button(cx,
            "load-references".into(),
            "加载引用内容",
            t.dim,
            cx.listener(|store, _, _, _| {
                for reference in crate::content::references(store) {
                    let _ = store
                        .requests()
                        .unbounded_send(BridgeRequest::ResolveContent {
                            seed: store.session_id.clone(),
                            reference,
                        });
                }
            }),
            t,
        ))
        .child(action_button(cx,
            "copy-conversation".into(),
            "复制对话",
            t.dim,
            cx.listener(|store, _, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(conversation_text(store)));
            }),
            t,
        ))
        .child(v_divider(t))
        .child(action_button(cx,
            "archive-session".into(),
            if archived { "取消归档" } else { "归档" },
            t.dim,
            cx.listener(move |store, _, _, _| {
                let _ = store.requests().unbounded_send(BridgeRequest::Command {
                    target: CommandTarget::Session {
                        session_id: store.session_id.clone(),
                    },
                    command: if archived {
                        RingingCommand::SessionUnarchive {}
                    } else {
                        RingingCommand::SessionArchive {}
                    },
                });
            }),
            t,
        ))
        .child(action_button(cx,
            "reload-agent".into(),
            "重载配置",
            t.dim,
            cx.listener(|store, _, _, _| {
                let _ = store.requests().unbounded_send(BridgeRequest::Command {
                    target: CommandTarget::Session {
                        session_id: store.session_id.clone(),
                    },
                    command: RingingCommand::AgentReloadConfig,
                });
            }),
            t,
        ))
        .child(action_button(cx,
            "goal-dialog".into(),
            "目标",
            t.dim,
            cx.listener(|store, _, window: &mut Window, cx| {
                crate::goal::open_dialog(store, window, cx);
                cx.notify();
            }),
            t,
        ))
        .child(div().flex_1())
        .child(action_button(cx,
            "shutdown-agent".into(),
            "停止会话",
            t.bad,
            cx.listener(|store, _, _, _| {
                let _ = store.requests().unbounded_send(BridgeRequest::Command {
                    target: CommandTarget::Session {
                        session_id: store.session_id.clone(),
                    },
                    command: RingingCommand::SessionShutdown,
                });
            }),
            t,
        ))
}

/// 控制条竖向 hairline 分隔。
fn v_divider(t: &Palette) -> Div {
    div().w(px(1.)).h(px(14.)).mx_1().bg(t.border)
}

pub fn conversation_text(store: &SessionStore) -> String {
    store
        .entries
        .iter()
        .filter_map(|entry| match entry {
            qaqh_ringing_contract::state::TimelineEntry::Input { content, .. }
            | qaqh_ringing_contract::state::TimelineEntry::AssistantBlock { content, .. } => {
                Some(crate::ui::content_text(content))
            }
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub fn composer_tools(store: &SessionStore, cx: &mut Context<SessionStore>, t: &Palette) -> Div {
    let mut tools = div().flex().flex_col().gap_1().p_2().text_xs().child(
        div()
            .flex()
            .gap_2()
            .child(action_button(cx, 
                "attach-file".into(),
                "图片 / 文本附件",
                t.accent,
                cx.listener(|store, _, _, cx| {
                    if store.attachment_pending {
                        return;
                    }
                    let picker = cx.prompt_for_paths(PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: false,
                        prompt: Some("选择图片或 UTF-8 文本（最大 4 MiB）".into()),
                    });
                    cx.spawn(async move |entity, cx| {
                        if let Ok(Ok(Some(paths))) = picker.await
                            && let Some(path) = paths.into_iter().next()
                        {
                            let _ = entity.update(cx, |store, cx| {
                                store.attachment_pending = true;
                                if store
                                    .requests()
                                    .unbounded_send(BridgeRequest::AttachFile {
                                        seed: store.session_id.clone(),
                                        path,
                                    })
                                    .is_err()
                                {
                                    store.attachment_pending = false;
                                    store.operation_error = Some("连接已关闭".into());
                                }
                                cx.notify();
                            });
                        }
                    })
                    .detach();
                }),
                t,
            ))
            .child(format!(
                "{} 张图片 · {} 个文本附件",
                store.images.len(),
                store.attachments.len()
            ))
            .child(action_button(cx, 
                "clear-attachments".into(),
                "移除附件",
                t.dim,
                cx.listener(|store, _, _, cx| {
                    store.images.clear();
                    store.attachments.clear();
                    cx.notify();
                }),
                t,
            )),
    );
    tools = tools.child(
        div()
            .flex()
            .flex_wrap()
            .gap_2()
            .children(store.images.iter().map(qaqh_gpui_render::media::preview)),
    );
    if let Some(outbox) = &store.outbox {
        tools = tools
            .child(format!("{} {}", outbox.state.label(), outbox.detail))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(cx, 
                        "check-receipt".into(),
                        "查回执",
                        t.accent,
                        cx.listener(|store, _, _, _| store.check_receipt()),
                        t,
                    ))
                    .children(
                        matches!(outbox.state, qaqh_gpui_wire::delivery::DeliveryState::Uncertain).then(
                            || {
                                action_button(cx, 
                                    "retry-message".into(),
                                    "重试原请求",
                                    t.warn,
                                    cx.listener(|store, _, _, cx| {
                                        store.retry_message();
                                        cx.notify();
                                    }),
                                    t,
                                )
                            },
                        ),
                    )
                    .children(
                        matches!(
                            outbox.state,
                            qaqh_gpui_wire::delivery::DeliveryState::Rejected
                                | qaqh_gpui_wire::delivery::DeliveryState::Failed
                        )
                        .then(|| {
                            action_button(cx, 
                                "restore-message".into(),
                                "恢复为草稿",
                                t.dim,
                                cx.listener(|store, _, window, cx| {
                                    if let Some(outbox) = store.outbox.take() {
                                        let text = outbox.message.text.clone();
                                        store.composer.update(cx, |composer, cx| {
                                            composer.set_value(text.as_str(), window, cx)
                                        });
                                        store.images = outbox.message.images;
                                        store.attachments =
                                            outbox.message.attachments.unwrap_or_default();
                                    }
                                    cx.notify();
                                }),
                                t,
                            )
                        }),
                    ),
            );
    }
    if let Some(error) = &store.operation_error {
        tools = tools.child(div().text_color(t.bad).child(error.clone()));
    }
    tools
}

pub fn search(
    store: &SessionStore,
    cx: &mut Context<SessionStore>,
) -> Option<gpui::AnyElement> {
    if !store.search_open {
        return None;
    }
    // Escape 关闭搜索(祖先层截获,先于输入引擎的 Escape 动作)。
    let input = div()
        .id("message-search-input")
        .on_key_down(cx.listener(|store, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                store.search_open = false;
                cx.stop_propagation();
                cx.notify();
            }
        }))
        .child(Input::new(&store.search).small());
    let query = store.search.read(cx).value().to_lowercase();
    let matches: Vec<_> = store
        .entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let text = match entry {
                qaqh_ringing_contract::state::TimelineEntry::Input { content, .. }
                | qaqh_ringing_contract::state::TimelineEntry::AssistantBlock { content, .. } => {
                    crate::ui::content_text(content)
                }
                _ => return None,
            };
            (!query.is_empty() && text.to_lowercase().contains(&query))
                .then_some((index, text.chars().take(70).collect::<String>()))
        })
        .collect();
    Some(
        div()
            .max_h(px(160.))
            .id("search-results")
            .overflow_y_scroll()
            .p_2()
            .child(input)
            .child(format!("{} 处匹配", matches.len()))
            .children(matches.into_iter().map(|(index, preview)| {
                div()
                    .id(("search-hit", index))
                    .cursor_pointer()
                    .child(preview)
                    .on_click(cx.listener(move |store, _, _, _| {
                        store.timeline_list.scroll_to_reveal_item(index)
                    }))
            }))
            .into_any_element(),
    )
}
