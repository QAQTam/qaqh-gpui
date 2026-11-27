//! 请求派发：域请求直通 + Shell 流副作用 + 名册/快照副作用。

use super::*;
use crate::bridge::attach::{prepare_attachment, review_text};
use crate::bridge::streams::attach_shell;

pub(super) async fn handle_request(
    client: &mut V3Client<YamuxTransport>,
    to_ui: &UnboundedSender<BridgeMsg>,
    generation: u64,
    shell_tasks: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    request: BridgeRequest,
) {
    let outcome: Result<ControlResponse, String> = match &request {
        BridgeRequest::Reconnect | BridgeRequest::Select { .. } => unreachable!("由连接属主处理"),
        BridgeRequest::RefreshRoster => {
            poll_roster(client, to_ui).await;
            return;
        }
        BridgeRequest::Refill(session_id) => {
            refill(client, to_ui, session_id).await;
            return;
        }
        BridgeRequest::LoadTranscript {
            session_id,
            before_ord,
            limit,
            timeline_epoch,
        } => {
            match call(
                client,
                ControlRequest::Session(SessionRequest::TranscriptPage {
                    session_id: session_id.clone(),
                    before_ord: *before_ord,
                    limit: *limit,
                    timeline_epoch: *timeline_epoch,
                }),
            )
            .await
            {
                Ok(ControlResponse::Session(SessionResponse::Page(page))) => {
                    let _ = to_ui.unbounded_send(BridgeMsg::Transcript(page));
                }
                Ok(other) => {
                    let message = format!("翻页回执形状异常：{other:?}");
                    send_line(to_ui, message.clone());
                    let _ = to_ui.unbounded_send(BridgeMsg::Session(Err(message)));
                }
                Err(error) => {
                    send_line(to_ui, format!("翻页失败：{error}"));
                    let _ = to_ui.unbounded_send(BridgeMsg::Session(Err(error)));
                }
            }
            return;
        }
        BridgeRequest::Session(request) => {
            call(client, ControlRequest::Session(request.clone())).await
        }
        BridgeRequest::Command {
            command_id,
            request,
        } => client
            .call_idempotent(command_id, ControlRequest::Command(request.clone()))
            .await
            .map_err(|error| error.to_string()),
        BridgeRequest::Config(request) => {
            call(client, ControlRequest::Config(request.clone())).await
        }
        BridgeRequest::Account(request) => {
            call(client, ControlRequest::Account(request.clone())).await
        }
        BridgeRequest::Workspace(request) => {
            call(client, ControlRequest::Workspace(request.clone())).await
        }
        BridgeRequest::ToolCatalog(request) => {
            call(client, ControlRequest::ToolCatalog(request.clone())).await
        }
        BridgeRequest::Shell(request) => {
            shell_request(client, to_ui, generation, shell_tasks, request.clone()).await;
            return;
        }
        BridgeRequest::ShellAttach {
            shell_id,
            from_offset,
        } => {
            if let Some(old) = shell_tasks.remove(shell_id) {
                old.abort();
            }
            match attach_shell(client, to_ui, generation, shell_id, *from_offset).await {
                Ok(task) => {
                    if let Some(old) = shell_tasks.insert(shell_id.clone(), task) {
                        old.abort();
                    }
                }
                Err(error) => {
                    send_shell_error(to_ui, format!("shell 流订阅失败（{shell_id}）：{error}"))
                }
            }
            return;
        }
        BridgeRequest::FetchBlob(hash) => {
            match client.fetch_blob(hash).await {
                Ok(bytes) => {
                    let _ = to_ui.unbounded_send(BridgeMsg::Blob {
                        hash: *hash,
                        bytes: bytes.to_vec(),
                    });
                }
                Err(error) => send_line(to_ui, format!("内容读取失败：{error}")),
            }
            return;
        }
        BridgeRequest::AttachFile { session_id, path } => {
            let result = prepare_attachment(client, session_id, path.clone()).await;
            let _ = to_ui.unbounded_send(BridgeMsg::Attachment {
                session_id: session_id.clone(),
                result,
            });
            return;
        }
        BridgeRequest::ReviewContent {
            session_id,
            turn,
            request,
            references,
        } => {
            let mut texts = Vec::with_capacity(references.len());
            for reference in references {
                texts.push(review_text(client, reference).await);
            }
            let _ = to_ui.unbounded_send(BridgeMsg::ReviewContent {
                session_id: session_id.clone(),
                turn: turn.clone(),
                request: *request,
                texts,
            });
            return;
        }
        BridgeRequest::Watchdog { label, after_ms } => {
            let to_ui = to_ui.clone();
            let label = label.clone();
            let after = *after_ms;
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(after)).await;
                let _ = to_ui.unbounded_send(BridgeMsg::Watchdog(label));
            });
            return;
        }
    };
    deliver(to_ui, &request, outcome);
}

/// Shell 域：`Open`/`Close`/`List` 带流副作用，其余（write/resize）直通。
async fn shell_request(
    client: &V3Client<YamuxTransport>,
    to_ui: &UnboundedSender<BridgeMsg>,
    generation: u64,
    shell_tasks: &mut HashMap<String, tokio::task::JoinHandle<()>>,
    request: ShellRequest,
) {
    match request {
        ShellRequest::Open {
            session_id,
            cols,
            rows,
            cwd,
            shell,
        } => {
            let opened = call(
                client,
                ControlRequest::Shell(ShellRequest::Open {
                    session_id,
                    cols,
                    rows,
                    cwd,
                    shell,
                }),
            )
            .await;
            match opened {
                Ok(ControlResponse::Shell(ShellResponse::Opened(view))) => {
                    // 先把页签交给 UI，再建流，避免首帧字节早于页签到达。
                    let _ = to_ui.unbounded_send(BridgeMsg::ShellOpened(view.clone()));
                    match attach_shell(client, to_ui, generation, &view.shell_id, 0).await {
                        Ok(task) => {
                            if let Some(old) = shell_tasks.insert(view.shell_id.clone(), task) {
                                old.abort();
                            }
                        }
                        Err(error) => send_shell_error(to_ui, format!("shell 流订阅失败：{error}")),
                    }
                }
                Ok(other) => send_shell_error(to_ui, format!("shell.open 回执异常：{other:?}")),
                Err(error) => send_shell_error(to_ui, format!("shell.open 失败：{error}")),
            }
        }
        ShellRequest::Close { shell_id } => {
            if let Some(old) = shell_tasks.remove(&shell_id) {
                old.abort();
            }
            let _ = to_ui.unbounded_send(BridgeMsg::ShellStatus {
                shell_id: shell_id.clone(),
                status: ShellUiStatus::Unsubscribed {
                    reason: "本地关闭".into(),
                },
            });
            let closed = call(
                client,
                ControlRequest::Shell(ShellRequest::Close {
                    shell_id: shell_id.clone(),
                }),
            )
            .await;
            match closed {
                Ok(ControlResponse::Shell(ShellResponse::Closed)) => {
                    let _ = to_ui.unbounded_send(BridgeMsg::ShellRemoved(shell_id));
                }
                Ok(other) => send_shell_error(to_ui, format!("shell.close 回执异常：{other:?}")),
                Err(error) => send_shell_error(to_ui, format!("shell.close 失败：{error}")),
            }
        }
        ShellRequest::List { session_id } => {
            let listed = call(
                client,
                ControlRequest::Shell(ShellRequest::List { session_id }),
            )
            .await;
            match listed {
                Ok(ControlResponse::Shell(ShellResponse::Shells(views))) => {
                    shell_tasks.retain(|id, handle| {
                        let keep = views.iter().any(|view| &view.shell_id == id);
                        if !keep {
                            handle.abort();
                        }
                        keep
                    });
                    let missing: Vec<ShellView> = views
                        .iter()
                        .filter(|view| !view.exited && !shell_tasks.contains_key(&view.shell_id))
                        .cloned()
                        .collect();
                    let _ = to_ui.unbounded_send(BridgeMsg::Shells(views));
                    for view in missing {
                        match attach_shell(
                            client,
                            to_ui,
                            generation,
                            &view.shell_id,
                            view.output_offset,
                        )
                        .await
                        {
                            Ok(task) => {
                                if let Some(old) = shell_tasks.insert(view.shell_id.clone(), task) {
                                    old.abort();
                                }
                            }
                            Err(error) => send_shell_error(
                                to_ui,
                                format!("shell 流订阅失败（{}）：{error}", view.shell_id),
                            ),
                        }
                    }
                }
                Ok(other) => send_shell_error(to_ui, format!("shell.list 回执异常：{other:?}")),
                Err(error) => send_shell_error(to_ui, format!("shell.list 失败：{error}")),
            }
        }
        other => {
            let outcome = call(client, ControlRequest::Shell(other)).await;
            let response = match outcome {
                Ok(ControlResponse::Shell(response)) => Ok(response),
                Ok(other) => Err(format!("shell 回执形状异常：{other:?}")),
                Err(error) => Err(error),
            };
            let _ = to_ui.unbounded_send(BridgeMsg::Shell(response));
        }
    }
}

/// 把域回执按请求种类投回 UI。
fn deliver(
    to_ui: &UnboundedSender<BridgeMsg>,
    request: &BridgeRequest,
    outcome: Result<ControlResponse, String>,
) {
    let mapped = match (request, outcome) {
        (_, Err(error)) => {
            send_line(to_ui, format!("请求失败：{error}"));
            match request {
                BridgeRequest::Session(_) => BridgeMsg::Session(Err(error)),
                BridgeRequest::Config(_) => BridgeMsg::Config(Err(error)),
                BridgeRequest::Account(_) => BridgeMsg::Account(Err(error)),
                BridgeRequest::Workspace(_) => BridgeMsg::Workspace(Err(error)),
                BridgeRequest::ToolCatalog(ToolCatalogRequest::Get { session_id }) => {
                    BridgeMsg::ToolCatalog {
                        session_id: session_id.clone(),
                        result: Err(error),
                    }
                }
                BridgeRequest::Shell(_) => BridgeMsg::Shell(Err(error)),
                BridgeRequest::Command { command_id, .. } => BridgeMsg::Command {
                    session_id: None,
                    command_id: command_id.clone(),
                    result: Err(error),
                },
                _ => return,
            }
        }
        (BridgeRequest::Session(_), Ok(ControlResponse::Session(response))) => {
            BridgeMsg::Session(Ok(response))
        }
        (BridgeRequest::Config(_), Ok(ControlResponse::Config(response))) => {
            BridgeMsg::Config(Ok(response))
        }
        (BridgeRequest::Account(_), Ok(ControlResponse::Account(response))) => {
            BridgeMsg::Account(Ok(response))
        }
        (BridgeRequest::Workspace(_), Ok(ControlResponse::Workspace(response))) => {
            BridgeMsg::Workspace(Ok(response))
        }
        (
            BridgeRequest::ToolCatalog(ToolCatalogRequest::Get { session_id }),
            Ok(ControlResponse::ToolCatalog(response)),
        ) => BridgeMsg::ToolCatalog {
            session_id: session_id.clone(),
            result: Ok(response),
        },
        (BridgeRequest::Shell(_), Ok(ControlResponse::Shell(response))) => {
            BridgeMsg::Shell(Ok(response))
        }
        (BridgeRequest::Command { command_id, request }, Ok(ControlResponse::Command(result))) => {
            let session_id = match request {
                CommandRequest::SendPrompt { session_id, .. }
                | CommandRequest::CancelTurn { session_id }
                | CommandRequest::RespondInteraction { session_id, .. } => Some(session_id.clone()),
                CommandRequest::ExecInterrupt { .. } | CommandRequest::ExecTerminate { .. } => None,
            };
            BridgeMsg::Command {
                session_id,
                command_id: command_id.clone(),
                result: Ok(result),
            }
        }
        (_, Ok(other)) => {
            send_line(to_ui, format!("回执形状异常：{other:?}"));
            return;
        }
    };
    let _ = to_ui.unbounded_send(mapped);
}

pub(super) async fn poll_roster(
    client: &V3Client<YamuxTransport>,
    to_ui: &UnboundedSender<BridgeMsg>,
) -> bool {
    let ok = match call(
        client,
        ControlRequest::Session(SessionRequest::List {
            include_archived: true,
        }),
    )
    .await
    {
        Ok(ControlResponse::Session(SessionResponse::Sessions(list))) => {
            trace(&format!("session.list {}", list.len()));
            let _ = to_ui.unbounded_send(BridgeMsg::Sessions(list));
            true
        }
        Ok(other) => {
            trace(&format!("session.list 回执异常：{other:?}"));
            false
        }
        Err(error) => {
            trace(&format!("session.list 失败：{error}"));
            false
        }
    };
    match call(client, ControlRequest::Workspace(WorkspaceRequest::List)).await {
        Ok(ControlResponse::Workspace(WorkspaceResponse::List(entries))) => {
            let _ = to_ui.unbounded_send(BridgeMsg::Workspaces(entries));
        }
        Ok(other) => trace(&format!("workspace.list 回执异常：{other:?}")),
        Err(error) => trace(&format!("workspace.list 失败：{error}")),
    }
    ok
}

/// 重灌：快照（尾部窗口）+ todo + team。
pub(super) async fn refill(
    client: &V3Client<YamuxTransport>,
    to_ui: &UnboundedSender<BridgeMsg>,
    session_id: &SessionId,
) {
    match call(
        client,
        ControlRequest::Session(SessionRequest::GetSessionSnapshot {
            session_id: session_id.clone(),
            tail_blocks: TAIL_BLOCKS,
            tail_turns: TAIL_TURNS,
        }),
    )
    .await
    {
        Ok(ControlResponse::Session(SessionResponse::Snapshot(snapshot))) => {
            trace(&format!(
                "snapshot {} seq={} blocks={} tools={} interactions={}",
                session_id.as_str(),
                snapshot.seq,
                snapshot.tail_blocks.len(),
                snapshot.tool_calls.len(),
                snapshot.pending_interactions.len(),
            ));
            let _ = to_ui.unbounded_send(BridgeMsg::Snapshot(snapshot));
        }
        Ok(other) => send_line(to_ui, format!("快照回执形状异常：{other:?}")),
        Err(error) => send_line(
            to_ui,
            format!("快照读取失败（{}）：{error}", session_id.as_str()),
        ),
    }
    for request in [
        SessionRequest::GetTodos {
            session_id: session_id.clone(),
        },
        SessionRequest::GetTeam {
            session_id: session_id.clone(),
        },
    ] {
        match call(client, ControlRequest::Session(request)).await {
            Ok(ControlResponse::Session(response)) => {
                let _ = to_ui.unbounded_send(BridgeMsg::Session(Ok(response)));
            }
            Ok(other) => trace(&format!("会话快照请求回执异常：{other:?}")),
            Err(error) => trace(&format!("会话快照请求失败：{error}")),
        }
    }
    match call(
        client,
        ControlRequest::ToolCatalog(ToolCatalogRequest::Get {
            session_id: session_id.clone(),
        }),
    )
    .await
    {
        Ok(ControlResponse::ToolCatalog(response)) => {
            let _ = to_ui.unbounded_send(BridgeMsg::ToolCatalog {
                session_id: session_id.clone(),
                result: Ok(response),
            });
        }
        Ok(other) => trace(&format!("工具目录请求回执异常：{other:?}")),
        Err(error) => trace(&format!("工具目录请求失败：{error}")),
    }
}
