//! Desktop lifecycle and controls, independent of timeline rendering.
use crate::{
    bridge::{BridgeRequest, ConnectionState},
    store::{DaemonStore, SessionRow, SessionStore},
    text_input::InputElement,
    theme::Palette,
    ui::{action_button, basic_field_key, field_box},
};
use gpui::{App, Context, Div, KeyDownEvent, PathPromptOptions, Window, div, prelude::*, px};
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

pub fn connection(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let label = match &daemon.connection {
        ConnectionState::Connecting => "正在连接 daemon…".to_string(),
        ConnectionState::Ready { .. } => "已连接".to_string(),
        ConnectionState::Failed { detail } => format!("连接失败：{detail}"),
    };
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .text_xs()
        .child(label)
        .child(
            div()
                .flex()
                .gap_2()
                .child(action_button(
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
                .child(action_button(
                    "diagnostics".into(),
                    "复制诊断",
                    t.dim,
                    cx.listener(|_, _, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                            crate::diagnostics::report(),
                        ));
                    }),
                    t,
                )),
        )
        .children(
            daemon
                .last_operation
                .clone()
                .map(|text| div().text_color(t.warn).child(text)),
        )
}

pub fn sidebar_tools(
    daemon: &DaemonStore,
    window: &Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let field = field_box(
        "session-search",
        &daemon.session_search_focus,
        t.border,
        cx.listener(|daemon, _, window, cx| window.focus(&daemon.session_search_focus, cx)),
        cx.listener(|daemon, event: &KeyDownEvent, _, cx| {
            basic_field_key(&mut daemon.session_search, event, cx)
        }),
    )
    .child(if daemon.session_search.text.is_empty() {
        "搜索会话…".into_element().into_any_element()
    } else {
        daemon
            .session_search
            .display(daemon.session_search_focus.is_focused(window), window)
            .into_any_element()
    });
    div()
        .p_2()
        .flex()
        .flex_col()
        .gap_2()
        .child(InputElement::new(
            cx.entity(),
            daemon.session_search_focus.clone(),
            field,
        ))
        .child(
            div()
                .flex()
                .flex_wrap()
                .gap_2()
                .child(action_button(
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
                .child(action_button(
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
                ))
                .child(action_button(
                    "sidebar-narrow".into(),
                    "窄",
                    t.dim,
                    cx.listener(|daemon, _, _, cx| {
                        daemon.workbench.sidebar_width =
                            Some((daemon.workbench.sidebar_width.unwrap_or(260.) - 20.).max(180.));
                        cx.notify();
                    }),
                    t,
                ))
                .child(action_button(
                    "sidebar-wide".into(),
                    "宽",
                    t.dim,
                    cx.listener(|daemon, _, _, cx| {
                        daemon.workbench.sidebar_width =
                            Some((daemon.workbench.sidebar_width.unwrap_or(260.) + 20.).min(420.));
                        cx.notify();
                    }),
                    t,
                )),
        )
}

pub fn welcome(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    div()
        .flex_1()
        .p_6()
        .flex()
        .flex_col()
        .gap_4()
        .child("欢迎使用 QAQHarness")
        .child("先在设置中添加服务商和模型，再选择工作目录并新建会话。")
        .child(connection(daemon, cx, t))
        .child(action_button(
            "setup-model".into(),
            "配置服务商与模型",
            t.accent,
            cx.listener(|daemon, _, _, cx| {
                daemon.settings_open = true;
                daemon.settings_editor = crate::store::SettingsEditor::Catalog;
                let _ = daemon.requests.unbounded_send(BridgeRequest::QueryConfig);
                let _ = daemon.requests.unbounded_send(BridgeRequest::ListProviders);
                cx.notify();
            }),
            t,
        ))
        .child(format!(
            "版本 {} · Ctrl+N 新建 · Ctrl+K 搜索会话 · Ctrl+F 搜索消息 · Ctrl+, 设置",
            env!("CARGO_PKG_VERSION")
        ))
}

pub fn session_controls(store: &SessionStore, cx: &mut Context<SessionStore>, t: &Palette) -> Div {
    let holder = store.driver.as_ref().and_then(|d| d.holder.clone());
    let own = holder.is_some() && holder == store.client_session_id;
    let archived = store.settings.as_ref().is_some_and(|s| s.meta.archived);
    let stale_holder = holder.clone();
    div()
        .flex()
        .flex_wrap()
        .gap_2()
        .p_2()
        .text_xs()
        .child(match holder {
            None => "控制权：空闲",
            Some(_) if own => "控制权：本客户端",
            _ => "控制权：其他客户端",
        })
        .child(action_button(
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
        .child(action_button(
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
        .child(action_button(
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
        .child(action_button(
            "shutdown-agent".into(),
            "停止会话",
            t.dim,
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
        .child(action_button(
            "message-search".into(),
            "搜索消息",
            t.dim,
            cx.listener(|store, _, window, cx| {
                store.search_open = !store.search_open;
                window.focus(&store.search_focus, cx);
                cx.notify();
            }),
            t,
        ))
        .child(action_button(
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
        .child(action_button(
            "copy-conversation".into(),
            "复制对话",
            t.dim,
            cx.listener(|store, _, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(conversation_text(store)));
            }),
            t,
        ))
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
            .child(action_button(
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
            .child(action_button(
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
            .children(store.images.iter().map(crate::media::preview)),
    );
    if let Some(outbox) = &store.outbox {
        tools = tools
            .child(format!("{} {}", outbox.state.label(), outbox.detail))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(action_button(
                        "check-receipt".into(),
                        "查回执",
                        t.accent,
                        cx.listener(|store, _, _, _| store.check_receipt()),
                        t,
                    ))
                    .children(
                        matches!(outbox.state, crate::delivery::DeliveryState::Uncertain).then(
                            || {
                                action_button(
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
                            crate::delivery::DeliveryState::Rejected
                                | crate::delivery::DeliveryState::Failed
                        )
                        .then(|| {
                            action_button(
                                "restore-message".into(),
                                "恢复为草稿",
                                t.dim,
                                cx.listener(|store, _, _, cx| {
                                    if let Some(outbox) = store.outbox.take() {
                                        store.composer = crate::text_input::TextField::with_text(
                                            &outbox.message.text,
                                        );
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
    window: &Window,
    cx: &mut Context<SessionStore>,
    t: &Palette,
) -> Option<gpui::AnyElement> {
    if !store.search_open {
        return None;
    }
    let input = field_box(
        "message-search-input",
        &store.search_focus,
        t.border,
        cx.listener(|store, _, window, cx| window.focus(&store.search_focus, cx)),
        cx.listener(|store, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                store.search_open = false;
            } else {
                basic_field_key(&mut store.search, event, cx);
            }
            cx.notify();
        }),
    )
    .child(
        store
            .search
            .display(store.search_focus.is_focused(window), window),
    );
    let query = store.search.text.to_lowercase();
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
            .child(InputElement::new(
                cx.entity(),
                store.search_focus.clone(),
                input,
            ))
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
