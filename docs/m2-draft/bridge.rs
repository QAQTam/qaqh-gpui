//! tokio ↔ gpui 桥(M2:命令面直发)。

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use qaqh_client::discovery::ensure_daemon_running;
use qaqh_client::discovery::read_discovery;
use qaqh_client::discovery::DiscoveryExt;
use qaqh_client::v2_stream::V2StreamStatus;
use qaqh_client::{
    Client, ClientHandlers, ClientOptions, CommandOptions, CommandTarget, ConversationSendMessage,
    InputPurpose, RingingCommand, RingingV2ResyncRequired,
};
use qaqh_ringing_contract::events::SessionEventFrame;
use qaqh_ringing_contract::state::SessionState;
use serde_json::json;

fn send_line(to_ui: &UnboundedSender<BridgeMsg>, text: &str) {
    let _ = to_ui.unbounded_send(BridgeMsg::Line(text.into()));
}

/// 桥 → GPUI 方向的消息。
pub enum BridgeMsg {
    /// 人类可读状态行(含命令受理/失败报告)。
    Line(String),
    /// `session.list` 结果(已 sanitize)。
    Sessions(serde_json::Value),
    /// SDK 托管流的业务事件帧。
    Event(String, SessionEventFrame),
    /// resync:前端清该会话暂态,再请求 Refill 重灌快照(D6)。
    Reset(String, RingingV2ResyncRequired),
    /// bootstrap 快照(初次或 reset 后重灌)。
    Snapshot(Box<SessionState>),
    /// 单流状态迁移(连接徽标)。
    Stream(String, V2StreamStatus),
    /// 字节层存活信号。
    Liveness,
}

/// GPUI → 桥方向的请求。
pub enum BridgeRequest {
    /// attach(session.resume)+ activate_stream + bootstrap 快照。
    Select(String),
    /// 重新 bootstrap 并投递快照(reset 后)。
    Refill(String),
    /// conversation.send_message。
    SendMessage { seed: String, text: String },
    /// tool.permission.respond。
    RespondPermission {
        seed: String,
        tool_call_id: String,
        approved: bool,
    },
    /// plan.review.respond。
    RespondPlan {
        seed: String,
        interaction_id: String,
        approved: bool,
    },
    /// conversation.cancel。
    Cancel {
        seed: String,
        turn_id: Option<String>,
    },
}

/// `--probe`:无 GUI 连通性验证,打印与窗口相同的状态行。
pub fn probe() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        run_connection(|line| println!("{line}")).await;
    });
}

/// 连接序列(M0 烟雾保留):discovery → connect → open_v2 → session.list。
pub async fn run_connection(report: impl Fn(String)) {
    report("qaqh-gpui:查找 daemon discovery…".into());

    let discovery = match read_discovery() {
        Ok(discovery) => {
            let url = match discovery.base_url() {
                Ok(url) => url,
                Err(error) => error.to_string(),
            };
            report(format!("discovery 就绪:{url}"));
            discovery
        }
        Err(error) => {
            report(format!("本地 discovery 不可读({error}),尝试拉起 daemon…"));
            match ensure_daemon_running(Duration::from_secs(6)) {
                Ok(discovery) => {
                    report(format!("daemon 已就绪:{}", discovery.base_url().unwrap_or_default()));
                    discovery
                }
                Err(error) => {
                    report(format!("daemon 不可达:{error}"));
                    return;
                }
            }
        }
    };

    let _ = discovery;
    let client = match Client::connect(ClientOptions {
        launch_daemon_if_missing: false,
        ..ClientOptions::default()
    }) {
        Ok(client) => client,
        Err(error) => {
            report(format!("连接失败:{error}"));
            return;
        }
    };
    report("client 已建立".into());

    let open = match client.open_v2().await {
        Ok(open) => open,
        Err(error) => {
            report(format!("v2 open 失败:{error}"));
            return;
        }
    };
    report(format!(
        "v2 open:client_session={},epoch={},commands={},session={}",
        open.client_session_id,
        open.server_epoch,
        open.command_contract_revision,
        open.session_contract_revision,
    ));

    match client
        .service_v2::<_, serde_json::Value>("session.list", json!({}))
        .await
    {
        Ok(value) => {
            let entries = qaqh_client::projection::sanitize_session_list(value);
            let count = entries.as_array().map(Vec::len).unwrap_or(0);
            report(format!("session.list:{count} 个会话"));
            for entry in entries.as_array().unwrap_or(&Vec::new()).iter().take(8) {
                report(format!(
                    "  · {} 「{}」",
                    entry["session_id"].as_str().unwrap_or("?"),
                    entry["title"].as_str().unwrap_or("(无标题)"),
                ));
            }
        }
        Err(error) => report(format!("session.list 失败:{error}")),
    }
}

/// 桥线程主循环:在私有 tokio 运行时里连接 daemon,挂四个回调,然后服务请求。
pub fn run(to_ui: UnboundedSender<BridgeMsg>, requests: UnboundedReceiver<BridgeRequest>) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(serve(to_ui, requests));
}

async fn serve(
    to_ui: UnboundedSender<BridgeMsg>,
    mut requests: UnboundedReceiver<BridgeRequest>,
) {
    send_line(&to_ui, "桥线程:查找 daemon discovery…");

    if let Err(error) = read_discovery() {
        send_line(
            &to_ui,
            &format!("本地 discovery 不可读({error}),尝试拉起 daemon…"),
        );
        if let Err(error) = ensure_daemon_running(Duration::from_secs(6)) {
            send_line(&to_ui, &format!("daemon 不可达:{error}"));
            return;
        }
    }

    let event_tx = to_ui.clone();
    let handlers = ClientHandlers {
        on_v2_event: Arc::new(move |seed: String, frame: SessionEventFrame| {
            let _ = event_tx.unbounded_send(BridgeMsg::Event(seed, frame));
        }),
        on_v2_reset: {
            let tx = to_ui.clone();
            Arc::new(move |seed: String, reset: RingingV2ResyncRequired| {
                let _ = tx.unbounded_send(BridgeMsg::Reset(seed, reset));
            })
        },
        on_v2_status: {
            let tx = to_ui.clone();
            Arc::new(move |seed: String, status: V2StreamStatus| {
                let _ = tx.unbounded_send(BridgeMsg::Stream(seed, status));
            })
        },
        on_liveness: {
            let tx = to_ui.clone();
            Arc::new(move || {
                let _ = tx.unbounded_send(BridgeMsg::Liveness);
            })
        },
    };

    let client = match Client::connect(ClientOptions {
        handlers,
        launch_daemon_if_missing: false,
        ..ClientOptions::default()
    }) {
        Ok(client) => client,
        Err(error) => {
            send_line(&to_ui, &format!("连接失败:{error}"));
            return;
        }
    };
    send_line(&to_ui, "client 已建立");

    let open = match client.open_v2().await {
        Ok(open) => open,
        Err(error) => {
            send_line(&to_ui, &format!("v2 open 失败:{error}"));
            return;
        }
    };
    send_line(
        &to_ui,
        &format!(
            "v2 open:client_session={},epoch={},commands={},session={}",
            open.client_session_id,
            open.server_epoch,
            open.command_contract_revision,
            open.session_contract_revision,
        ),
    );

    match client
        .service_v2::<_, serde_json::Value>("session.list", json!({}))
        .await
    {
        Ok(value) => {
            let count = value.as_array().map(Vec::len).unwrap_or(0);
            send_line(&to_ui, &format!("session.list:{count} 个会话"));
            let _ = to_ui.unbounded_send(BridgeMsg::Sessions(
                qaqh_client::projection::sanitize_session_list(value),
            ));
        }
        Err(error) => send_line(&to_ui, &format!("session.list 失败:{error}")),
    }

    while let Some(request) = requests.next().await {
        match request {
            BridgeRequest::Select(seed) => {
                if let Err(error) = client.attach(&seed).await {
                    send_line(&to_ui, &format!("attach {seed} 失败:{error}"));
                    continue;
                }
                client.activate_stream(&seed).await;
                refill(&client, &to_ui, &seed).await;
            }
            BridgeRequest::Refill(seed) => refill(&client, &to_ui, &seed).await,
            BridgeRequest::SendMessage { seed, text } => {
                send_command(
                    &client,
                    &to_ui,
                    &seed,
                    RingingCommand::ConversationSendMessage(ConversationSendMessage {
                        text,
                        images: Vec::new(),
                        attachments: None,
                        message_id: None,
                        input_purpose: InputPurpose::default(),
                        as_system: false,
                        inter_agent: None,
                        subagent_terminal: None,
                    }),
                )
                .await;
            }
            BridgeRequest::RespondPermission {
                seed,
                tool_call_id,
                approved,
            } => {
                send_command(
                    &client,
                    &to_ui,
                    &seed,
                    RingingCommand::ToolPermissionRespond {
                        tool_call_id,
                        approved,
                        trust_folder: false,
                    },
                )
                .await;
            }
            BridgeRequest::RespondPlan {
                seed,
                interaction_id,
                approved,
            } => {
                send_command(
                    &client,
                    &to_ui,
                    &seed,
                    RingingCommand::PlanReviewRespond {
                        interaction_id,
                        approved,
                        message: None,
                        autonomous: false,
                    },
                )
                .await;
            }
            BridgeRequest::Cancel { seed, turn_id } => {
                send_command(
                    &client,
                    &to_ui,
                    &seed,
                    RingingCommand::ConversationCancel { turn_id },
                )
                .await;
            }
        }
    }
}

/// 命令直发(target = Session);ack/错误如实上报。
async fn send_command(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    seed: &str,
    command: RingingCommand,
) {
    let target = CommandTarget::Session {
        session_id: seed.to_string(),
    };
    match client
        .send_command_v2_typed(target, command, CommandOptions::default())
        .await
    {
        Ok(ack) => send_line(
            to_ui,
            &format!("命令 {} 受理:{:?}", ack.command_id, ack.status),
        ),
        Err(error) => send_line(to_ui, &format!("命令发送失败:{error}")),
    }
}

async fn refill(client: &Client, to_ui: &UnboundedSender<BridgeMsg>, seed: &str) {
    match client.bootstrap_v2(seed).await {
        Ok(bootstrap) => {
            let _ = to_ui.unbounded_send(BridgeMsg::Snapshot(Box::new(bootstrap.state)));
        }
        Err(error) => {
            let _ = to_ui.unbounded_send(BridgeMsg::Line(format!(
                "bootstrap {seed} 失败:{error}"
            )));
        }
    }
}
