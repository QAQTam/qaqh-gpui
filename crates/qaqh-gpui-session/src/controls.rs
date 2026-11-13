//! Desktop lifecycle and controls, independent of timeline rendering.
use qaqh_gpui_wire::bridge::{BridgeRequest, ConnectionState};
use qaqh_gpui_kit::theme::Palette;
use crate::store::SessionStore;
use qaqh_gpui_kit::primitives::action_button;
use gpui::{App, Context, Div, FontWeight, KeyDownEvent, PathPromptOptions, SharedString, Window, div, prelude::*, px};
use gpui_component::input::Input;
use gpui_component::Sizable;
use qaqh_client::{CommandTarget, RingingCommand, SessionListEntry};
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
                Some(crate::timeline::content_text(content))
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
                    crate::timeline::content_text(content)
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