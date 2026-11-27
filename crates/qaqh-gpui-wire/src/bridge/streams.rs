//! 三条长活流：会话事件 / Shell PTY / 服务端主动推的 Blob。
//!
//! 每个任务只持有自己的流（不需要 `&V3Client`），因此连接重建时只需 abort 句柄；
//! 回调统一经 `FromConnection(generation, ..)` 带代际，旧连接的残余帧会被 UI 丢弃。

use super::*;
use qaqh_client::v3::BlobInbox;

pub(super) async fn attach_events(
    client: &V3Client<YamuxTransport>,
    to_ui: &UnboundedSender<BridgeMsg>,
    generation: u64,
    session_id: &SessionId,
    since_seq: u64,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let mut stream = client
        .attach_session(session_id, since_seq)
        .await
        .map_err(|error| error.to_string())?;
    let to_ui = to_ui.clone();
    let seed = session_id.clone();
    Ok(tokio::spawn(async move {
        from_connection(
            &to_ui,
            generation,
            BridgeMsg::Stream(
                seed.clone(),
                StreamProgress {
                    last_seq: stream.attach().seq,
                    ..StreamProgress::default()
                },
            ),
        );
        loop {
            match stream.next_frame().await {
                Ok(frame) => {
                    trace(&format!("event {} seq={}", seed.as_str(), frame.meta.seq));
                    from_connection(&to_ui, generation, BridgeMsg::Event(seed.clone(), frame));
                }
                Err(error) => {
                    let stats = stream.stats();
                    from_connection(
                        &to_ui,
                        generation,
                        BridgeMsg::Stream(
                            seed.clone(),
                            StreamProgress {
                                frames: stats.frames,
                                last_seq: stats.last_seq,
                                resync_cycles: stats.resync_cycles,
                                ordering_violations: stats.ordering_violations,
                                ended: true,
                            },
                        ),
                    );
                    from_connection(
                        &to_ui,
                        generation,
                        BridgeMsg::Line(format!("会话事件流结束（{}）：{error}", seed.as_str())),
                    );
                    return;
                }
            }
        }
    }))
}

pub(super) async fn attach_shell(
    client: &V3Client<YamuxTransport>,
    to_ui: &UnboundedSender<BridgeMsg>,
    generation: u64,
    shell_id: &str,
    from_offset: u64,
) -> Result<tokio::task::JoinHandle<()>, String> {
    let mut stream = client
        .attach_shell(shell_id, from_offset)
        .await
        .map_err(|error| error.to_string())?;
    let to_ui = to_ui.clone();
    let id = shell_id.to_string();
    Ok(tokio::spawn(async move {
        from_connection(
            &to_ui,
            generation,
            BridgeMsg::ShellStatus {
                shell_id: id.clone(),
                status: ShellUiStatus::Open,
            },
        );
        loop {
            match stream.next_frame().await {
                Ok(frame) => from_connection(
                    &to_ui,
                    generation,
                    BridgeMsg::ShellBytes {
                        shell_id: id.clone(),
                        bytes: frame.bytes.to_vec(),
                    },
                ),
                Err(error) => {
                    from_connection(
                        &to_ui,
                        generation,
                        BridgeMsg::ShellStatus {
                            shell_id: id.clone(),
                            status: ShellUiStatus::Terminated {
                                reason: error.to_string(),
                            },
                        },
                    );
                    return;
                }
            }
        }
    }))
}

/// 消费服务端主动推来的 blob（收件箱由 client 一次性交给本任务）。
pub(super) fn spawn_blob_inbox(
    to_ui: &UnboundedSender<BridgeMsg>,
    generation: u64,
    mut inbox: BlobInbox,
) -> tokio::task::JoinHandle<()> {
    let to_ui = to_ui.clone();
    tokio::spawn(async move {
        while let Some(blob) = inbox.next().await {
            from_connection(
                &to_ui,
                generation,
                BridgeMsg::Blob {
                    hash: blob.hash,
                    bytes: blob.bytes,
                },
            );
        }
    })
}
