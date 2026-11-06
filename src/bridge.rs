//! tokio ↔ gpui 桥(M3:命令面直发 + 多会话事件路由 + 名册轮询)。

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use qaqh_client::discovery::ensure_daemon_running;
use qaqh_client::discovery::read_discovery;
use qaqh_client::discovery::DiscoveryExt;
use qaqh_client::v2_stream::V2StreamStatus;
use qaqh_client::{
    AskAnswer, Client, ClientHandlers, ClientOptions, CommandOptions, CommandPrecondition,
    CommandTarget, ConversationSendMessage, InputPurpose, RingingCommand,
    RingingV2ResyncRequired, SessionSettingsPatch,
};
use qaqh_client::ringing_v2::RingingV2CommandAck;
use qaqh_types::ModelSelection;
use qaqh_types::SessionListEntry;
use qaqh_config_api::{ConfigDto, ConfigPatch};
use qaqh_profile::ProfilesDocument;
use qaqh_ringing_contract::events::SessionEventFrame;
use qaqh_ringing_contract::fact::{ContentRef, TurnId};
use qaqh_ringing_contract::state::SessionState;
use qaqh_ringing_contract::workspace::WorkspaceEntry;
use serde_json::json;

fn send_line(to_ui: &UnboundedSender<BridgeMsg>, text: &str) {
    let _ = to_ui.unbounded_send(BridgeMsg::Line(text.into()));
}

/// 桥 → GPUI 方向的消息。
pub enum BridgeMsg {
    /// 人类可读状态行(含命令受理/失败报告)。
    Line(String),
    /// `session.list` 原始数组(前端自行投影 status/workspace_id)。
    Sessions(serde_json::Value),
    /// `workspace.list` 条目(侧栏分组)。
    Workspaces(Vec<WorkspaceEntry>),
    /// 当前会话的 `todo.list` 快照。
    Todos(String, serde_json::Value),
    /// SDK 托管流的业务事件帧。
    Event(String, SessionEventFrame),
    /// resync:前端清该会话暂态,再请求 Refill 重灌快照(D6)。
    Reset(String, RingingV2ResyncRequired),
    /// bootstrap 快照(初次或 reset 后重灌)。
    Snapshot(Box<SessionState>),
    /// 单流状态迁移(连接徽标)。
    Stream(String, V2StreamStatus),
    /// session.create 同步完成:新会话 id(ack.result.session_id)。
    Created(String, Option<String>),
    CreateFailed,
    ReviewContent(String, TurnId, usize, Vec<Result<String, String>>),
    Config(ConfigDto),
    /// provider.list 的权威目录。
    Providers(ProfilesDocument),
    /// 取消看门狗到点:该会话仍未回终态(electron CANCEL_WATCHDOG 模式)。
    Watchdog(String),
    SessionSettings {
        seed: String,
        settings: Option<SessionListEntry>,
        write_completed: bool,
    },
    /// 字节层存活信号。
    Liveness,
}

/// 测试可观测性:`QAQH_GPUI_TRACE=1` 时把协议层动态打到 stderr。
fn trace(msg: &str) {
    if std::env::var_os("QAQH_GPUI_TRACE").is_some() {
        eprintln!("[qaqh-gpui] {msg}");
    }
}

/// GPUI → 桥方向的请求。
pub enum BridgeRequest {
    ReviewContent {
        session_id: String,
        turn: TurnId,
        request: usize,
        references: Vec<ContentRef>,
    },
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
        /// "始终允许" = 信任当前工作区目录。
        trust_folder: bool,
    },
    /// plan.review.respond。
    RespondPlan {
        seed: String,
        interaction_id: String,
        approved: bool,
    },
    /// interaction.ask.respond(answers 已按题 id 组装)。
    RespondAsk {
        seed: String,
        interaction_id: String,
        answers: Vec<AskAnswer>,
    },
    /// interaction.ask.dismiss。
    DismissAsk { seed: String, interaction_id: String },
    /// exec_session.interrupt / exec_session.terminate(target = ExecSession)。
    ExecControl { exec_id: String, terminate: bool },
    /// session.rename。
    RenameSession { seed: String, title: String },
    /// todo.list 查询(当前会话)。
    QueryTodo { seed: String },
    /// conversation.cancel。
    Cancel {
        seed: String,
        turn_id: Option<String>,
    },
    /// session.create(cwd = 目标工作区路径)。
    CreateSession {
        cwd: Option<String>,
        model_selection: ModelSelection,
    },
    PatchSessionSettings {
        seed: String,
        patch: SessionSettingsPatch,
        revision: u64,
    },
    /// workspace.move_session(把该会话移入目标分组)。
    MoveSession { seed: String, workspace_id: String },
    /// workspace.detach(该会话脱离分组)。
    DetachSession { seed: String },
    QuerySessionSettings(String),
    SaveConfig(ConfigPatch),
    ManageCatalog {
        command: RingingCommand,
        revision: u64,
    },
    /// provider.list(命令;结果在 ack.result)。
    ListProviders,
    /// config.load 查询。
    QueryConfig,
    /// session.delete(名册轮询自然刷新列表)。
    DeleteSession { seed: String },
    /// 取消看门狗:after_ms 后仍无终态则通知 UI 本地收口。
    Watchdog { seed: String, after_ms: u64 },
}

/// `--probe`:无 GUI 连通性验证,打印与窗口相同的状态行。
/// 复用 qaqh-client 的全局 tokio 运行时(SDK connect 内部会 block_on,
/// 自建 runtime 会"runtime inside runtime"崩溃)。
pub fn probe() {
    qaqh_client::runtime_handle().block_on(async {
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
    let client = match Client::connect_async(ClientOptions {
        launch_daemon_if_missing: false,
        ..ClientOptions::default()
    })
    .await
    {
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

/// 桥线程主循环:复用 qaqh-client 的全局 tokio 运行时,挂四个回调,服务请求。
pub fn run(to_ui: UnboundedSender<BridgeMsg>, requests: UnboundedReceiver<BridgeRequest>) {
    qaqh_client::runtime_handle().block_on(serve(to_ui, requests));
}

async fn serve(
    to_ui: UnboundedSender<BridgeMsg>,
    mut requests: UnboundedReceiver<BridgeRequest>,
) {
    send_line(&to_ui, "桥线程:查找 daemon discovery…");

    // daemon 可能正在启动/重启,或 discovery 是强杀遗留的 stale 文件(SDK 会
    // 判其为缺失)。重试必须包住完整 connect+open+list,失败才重试:12×5s=60s。
    let mut client = None;
    for attempt in 1..=12 {
        match establish(&to_ui).await {
            Ok(established) => {
                client = Some(established);
                break;
            }
            Err(error) => {
                trace(&format!("建立失败({error}),第 {attempt}/12 次重试"));
                send_line(&to_ui, &format!("daemon 未就绪({error}),5s 后重试…"));
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
    let Some(client) = client else {
        send_line(&to_ui, "daemon 不可达:60s 内未能建立连接");
        return;
    };

    poll_roster(&client, &to_ui).await;
    query_providers(&client, &to_ui).await;

    // 名册轮询(session.list + workspace.list):侧栏状态点与分组保持新鲜。
    let mut roster = tokio::time::interval(Duration::from_secs(15));
    loop {
        tokio::select! {
            request = requests.next() => {
                let Some(request) = request else { break };
                handle_request(&client, &to_ui, request).await;
            }
            _ = roster.tick() => poll_roster(&client, &to_ui).await,
        }
    }
}

/// 完整建立:discovery(缺失则尝试拉起)→ connect → open_v2 → session.list。
async fn establish(to_ui: &UnboundedSender<BridgeMsg>) -> Result<Client, qaqh_client::ClientError> {
    if read_discovery().is_err() {
        // Err(error) 只用于日志:SDK 对 stale/缺失一律走拉起路径。
        if let Err(error) = ensure_daemon_running(Duration::from_secs(4)) {
            trace(&format!("拉起未成:{error}"));
        }
    }

    let event_tx = to_ui.clone();
    let handlers = ClientHandlers {
        on_v2_event: Arc::new(move |seed: String, frame: SessionEventFrame| {
            if let Ok(payload) = serde_json::to_string(&frame.event) {
                let head: String = payload.chars().take(140).collect();
                trace(&format!("event seed={seed} seq={} {head}", frame.seq));
            }
            let _ = event_tx.unbounded_send(BridgeMsg::Event(seed, frame));
        }),
        on_v2_reset: {
            let tx = to_ui.clone();
            Arc::new(move |seed: String, reset: RingingV2ResyncRequired| {
                trace(&format!("reset seed={seed} {reset:?}"));
                let _ = tx.unbounded_send(BridgeMsg::Reset(seed, reset));
            })
        },
        on_v2_status: {
            let tx = to_ui.clone();
            Arc::new(move |seed: String, status: V2StreamStatus| {
                trace(&format!("stream seed={seed} {status:?}"));
                let _ = tx.unbounded_send(BridgeMsg::Stream(seed, status));
            })
        },
        on_liveness: {
            let tx = to_ui.clone();
            Arc::new(move || {
                trace("liveness 字节到达");
                let _ = tx.unbounded_send(BridgeMsg::Liveness);
            })
        },
    };

    let client = Client::connect_async(ClientOptions {
        handlers,
        launch_daemon_if_missing: false,
        ..ClientOptions::default()
    })
    .await?;
    send_line(to_ui, "client 已建立");

    let open = client.open_v2().await?;
    send_line(
        to_ui,
        &format!(
            "v2 open:client_session={},epoch={},commands={},session={}",
            open.client_session_id,
            open.server_epoch,
            open.command_contract_revision,
            open.session_contract_revision,
        ),
    );
    trace(&format!(
        "open csid={} epoch={} commands={} session={}",
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
            let count = value.as_array().map(Vec::len).unwrap_or(0);
            send_line(to_ui, &format!("session.list:{count} 个会话"));
            trace(&format!("session.list {count} 条"));
            // 原始数组直传:前端投影需要 status/workspace_id(sanitize 会丢)。
            let _ = to_ui.unbounded_send(BridgeMsg::Sessions(value));
        }
        Err(error) => send_line(to_ui, &format!("session.list 失败:{error}")),
    }
    Ok(client)
}

async fn handle_request(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    request: BridgeRequest,
) {
    match request {
        BridgeRequest::ReviewContent { session_id, turn, request, references } => {
            let client = client.clone();
            let to_ui = to_ui.clone();
            tokio::spawn(async move {
                let mut results = Vec::with_capacity(references.len());
                for reference in references {
                    let result = match client.download_content(&reference).await {
                        Ok(bytes) => String::from_utf8(bytes).map_err(|error| error.to_string()),
                        Err(error) => Err(error.to_string()),
                    };
                    results.push(result);
                }
                let _ = to_ui.unbounded_send(BridgeMsg::ReviewContent(session_id, turn, request, results));
            });
        }
        BridgeRequest::Select(seed) => {
            trace(&format!("select {seed}:attach+activate+bootstrap"));
            if let Err(error) = client.attach(&seed).await {
                trace(&format!("attach {seed} 失败:{error}"));
                send_line(to_ui, &format!("attach {seed} 失败:{error}"));
                return;
            }
            client.activate_stream(&seed).await;
            refill(client, to_ui, &seed).await;
            query_todos(client, to_ui, &seed).await;
        }
        BridgeRequest::Refill(seed) => refill(client, to_ui, &seed).await,
        BridgeRequest::QuerySessionSettings(seed) => query_session_settings(client, to_ui, &seed, false).await,
        BridgeRequest::SendMessage { seed, text } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
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
            trust_folder,
        } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::ToolPermissionRespond {
                    tool_call_id,
                    approved,
                    trust_folder,
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
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::PlanReviewRespond {
                    interaction_id,
                    approved,
                    message: None,
                    autonomous: false,
                },
            )
            .await;
        }
        BridgeRequest::RespondAsk {
            seed,
            interaction_id,
            answers,
        } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::InteractionAskRespond {
                    interaction_id,
                    answers,
                },
            )
            .await;
        }
        BridgeRequest::DismissAsk {
            seed,
            interaction_id,
        } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::InteractionAskDismiss { interaction_id },
            )
            .await;
        }
        BridgeRequest::ExecControl { exec_id, terminate } => {
            let command = if terminate {
                RingingCommand::ExecSessionTerminate {}
            } else {
                RingingCommand::ExecSessionInterrupt {}
            };
            send_command(
                client,
                to_ui,
                CommandTarget::ExecSession {
                    id: qaqh_ringing_contract::fact::ExecSessionId(exec_id),
                },
                command,
            )
            .await;
        }
        BridgeRequest::RenameSession { seed, title } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::SessionRename { title },
            )
            .await;
        }
        BridgeRequest::QueryTodo { seed } => query_todos(client, to_ui, &seed).await,
        BridgeRequest::Cancel { seed, turn_id } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::ConversationCancel { turn_id },
            )
            .await;
        }
        BridgeRequest::CreateSession { cwd, model_selection } => {
            let command = RingingCommand::SessionCreate {
                cwd: cwd.clone(),
                title: None,
                profile: None,
                effort: None,
                model_selection: Some(model_selection),
                tool_mode: None,
                custom_tools: None,
            };
            let result = send_command(client, to_ui, CommandTarget::Global, command)
                .await
                .and_then(|ack| ack.result)
                .and_then(|result| result["session_id"].as_str().map(str::to_owned));
            let message = match result {
                Some(session_id) => BridgeMsg::Created(session_id, cwd),
                None => BridgeMsg::CreateFailed,
            };
            let _ = to_ui.unbounded_send(message);
        }
        BridgeRequest::PatchSessionSettings { seed, patch, revision } => {
            let result = client
                .send_command_v2_typed(
                    CommandTarget::Session { session_id: seed.clone() },
                    RingingCommand::SessionSettingsPatch { patch },
                    CommandOptions {
                        preconditions: vec![CommandPrecondition::SettingsRevision { revision }],
                        ..Default::default()
                    },
                )
                .await;
            match result {
                Ok(ack) => send_line(to_ui, &format!("会话设置: {:?} {:?}", ack.status, ack.code)),
                Err(error) => send_line(to_ui, &format!("会话设置保存失败: {error}")),
            }
            query_session_settings(client, to_ui, &seed, true).await;
        }
        BridgeRequest::MoveSession { seed, workspace_id } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::WorkspaceMoveSession { workspace_id },
            )
            .await;
        }
        BridgeRequest::DetachSession { seed } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::WorkspaceDetach {},
            )
            .await;
        }
        BridgeRequest::SaveConfig(patch) => {
            match serde_json::to_value(patch) {
                Ok(patch) => {
                    send_command(client, to_ui, CommandTarget::Global, RingingCommand::ConfigSave { patch }).await;
                }
                Err(error) => send_line(to_ui, &format!("配置编码失败: {error}")),
            }
            query_config(client, to_ui).await;
        }
        BridgeRequest::ManageCatalog { command, revision } => {
            let options = CommandOptions {
                preconditions: if command.requires_config_revision() {
                    vec![CommandPrecondition::ConfigRevision { revision }]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let name = command.name();
            match client.send_command_v2_typed(CommandTarget::Global, command, options).await {
                Ok(ack) => send_line(to_ui, &format!("{name}: {:?} {:?}", ack.status, ack.code)),
                Err(error) => send_line(to_ui, &format!("{name} 失败: {error}")),
            }
            query_providers(client, to_ui).await;
            query_config(client, to_ui).await;
        }
        BridgeRequest::ListProviders => query_providers(client, to_ui).await,

        BridgeRequest::QueryConfig => query_config(client, to_ui).await,
        BridgeRequest::DeleteSession { seed } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::SessionDelete {},
            )
            .await;
        }
        BridgeRequest::Watchdog { seed, after_ms } => {
            // 到点后由桥线程投递;UI 侧自行判断该会话是否仍在运行。
            let tx = to_ui.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(after_ms)).await;
                let _ = tx.unbounded_send(BridgeMsg::Watchdog(seed));
            });
        }
    }
}

async fn query_providers(client: &Client, to_ui: &UnboundedSender<BridgeMsg>) {
    let result = client.send_command_v2_typed(
        CommandTarget::Global,
        RingingCommand::ProviderList {},
        CommandOptions::default(),
    ).await;
    let result = match result {
        Ok(ack) => match ack.result {
            Some(result) => result,
            None => {
                send_line(to_ui, &format!("模型目录读取失败: {:?} {:?}", ack.status, ack.code));
                return;
            }
        },
        Err(error) => {
            send_line(to_ui, &format!("模型目录读取失败: {error}"));
            return;
        }
    };
    match serde_json::from_value(result) {
        Ok(catalog) => {
            let _ = to_ui.unbounded_send(BridgeMsg::Providers(catalog));
        }
        Err(error) => send_line(to_ui, &format!("模型目录解析失败: {error}")),
    }
}

async fn query_config(client: &Client, to_ui: &UnboundedSender<BridgeMsg>) {
    match client.service_v2("config.load", json!({})).await {
        Ok(config) => { let _ = to_ui.unbounded_send(BridgeMsg::Config(config)); }
        Err(error) => send_line(to_ui, &format!("config.load 失败: {error}")),
    }
}

async fn query_session_settings(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    seed: &str,
    write_completed: bool,
) {
    let meta = match client.service_v2("session.meta", json!({ "session_id": seed })).await {
        Ok(meta) => meta,
        Err(error) => {
            send_line(to_ui, &format!("session.meta 失败: {error}"));
            None
        }
    };
    let _ = to_ui.unbounded_send(BridgeMsg::SessionSettings {
        seed: seed.into(),
        settings: meta,
        write_completed,
    });
}

/// 名册轮询:session.list(原始)+ workspace.list(typed)。
async fn poll_roster(client: &Client, to_ui: &UnboundedSender<BridgeMsg>) {
    match client
        .service_v2::<_, serde_json::Value>("session.list", json!({}))
        .await
    {
        Ok(value) => {
            trace(&format!("session.list {}", value.as_array().map(Vec::len).unwrap_or(0)));
            let _ = to_ui.unbounded_send(BridgeMsg::Sessions(value));
        }
        Err(error) => trace(&format!("session.list 失败:{error}")),
    }
    match client
        .service_v2::<_, Vec<WorkspaceEntry>>("workspace.list", json!({}))
        .await
    {
        Ok(entries) => {
            trace(&format!("workspace.list {} 组", entries.len()));
            let _ = to_ui.unbounded_send(BridgeMsg::Workspaces(entries));
        }
        Err(error) => trace(&format!("workspace.list 失败:{error}")),
    }
}

/// todo.list 查询;失败只 trace,UI 保留旧快照。
async fn query_todos(client: &Client, to_ui: &UnboundedSender<BridgeMsg>, seed: &str) {
    match client
        .service_v2::<_, serde_json::Value>("todo.list", json!({ "session_id": seed }))
        .await
    {
        Ok(value) => {
            let _ = to_ui.unbounded_send(BridgeMsg::Todos(seed.to_string(), value));
        }
        Err(error) => trace(&format!("todo.list {seed} 失败:{error}")),
    }
}

/// 命令直发;ack/错误如实上报,ack 交回调用方(result/revision 载荷)。
async fn send_command(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    target: CommandTarget,
    command: RingingCommand,
) -> Option<RingingV2CommandAck> {
    match client
        .send_command_v2_typed(target, command, CommandOptions::default())
        .await
    {
        Ok(ack) => {
            trace(&format!(
                "command {} 受理 {:?}",
                ack.command_id, ack.status
            ));
            send_line(
                to_ui,
                &format!("命令 {}: {:?} {:?}", ack.command_id, ack.status, ack.code),
            );
            Some(ack)
        }
        Err(error) => {
            trace(&format!("command 发送失败 {error}"));
            send_line(to_ui, &format!("命令发送失败:{error}"));
            None
        }
    }
}

async fn refill(client: &Client, to_ui: &UnboundedSender<BridgeMsg>, seed: &str) {
    query_session_settings(client, to_ui, seed, false).await;
    match client.bootstrap_v2(seed).await {
        Ok(bootstrap) => {
            trace(&format!(
                "snapshot seed={seed} seq={} turns={} timeline={} tools={} interactions={} exec={}",
                bootstrap.state.seq,
                bootstrap.state.turns.len(),
                bootstrap.state.timeline.len(),
                bootstrap.state.tools.len(),
                bootstrap.state.interactions.len(),
                bootstrap.state.exec_sessions.len(),
            ));
            let _ = to_ui.unbounded_send(BridgeMsg::Snapshot(Box::new(bootstrap.state)));
        }
        Err(error) => {
            trace(&format!("bootstrap {seed} 失败:{error}"));
            let _ = to_ui.unbounded_send(BridgeMsg::Line(format!(
                "bootstrap {seed} 失败:{error}"
            )));
        }
    }
}
