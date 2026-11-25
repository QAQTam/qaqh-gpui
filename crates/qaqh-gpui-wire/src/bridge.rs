//! tokio ↔ gpui 桥(M3:命令面直发 + 多会话事件路由 + 名册轮询)。

use std::sync::Arc;
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use qaqh_client::discovery::ensure_daemon_running;
use qaqh_client::discovery::read_discovery;
use qaqh_client::discovery::DiscoveryExt;
use qaqh_client::v2_stream::V2StreamStatus;
use qaqh_client::shell_stream::{ShellStream, ShellStreamHandlers, ShellStreamStatus};
use qaqh_client::ringing::ShellStreamEvent;
use std::collections::HashMap;
use qaqh_client::{
    AskAnswer, Client, ClientHandlers, ClientOptions, CommandOptions, CommandPrecondition,
    CommandTarget, QueryRequest, RingingCommand,
    RingingV2ResyncRequired, SessionSettingsPatch,
};
use qaqh_client::ringing_v2::RingingV2CommandAck;
use qaqh_types::ModelSelection;
use qaqh_types::SessionListEntry;
use qaqh_config_api::{ConfigDto, ConfigPatch};
use qaqh_profile::ProfilesDocument;
use qaqh_ringing_contract::events::SessionEventFrame;
use qaqh_ringing_contract::fact::{ContentRef, PermissionGrantScope, TurnId};
use qaqh_ringing_contract::state::SessionState;
use qaqh_ringing_contract::workspace::WorkspaceEntry;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static CONNECTION_GENERATION: AtomicU64 = AtomicU64::new(0);
pub fn connection_generation() -> u64 { CONNECTION_GENERATION.load(Ordering::Acquire) }
fn from_connection(sender: &UnboundedSender<BridgeMsg>, generation: u64, message: BridgeMsg) {
    let _ = sender.unbounded_send(BridgeMsg::FromConnection(generation, Box::new(message)));
}

#[derive(Clone)]
pub enum ConnectionState {
    Connecting,
    Ready { client_session_id: String },
    Failed { detail: String },
}
pub enum DeliveryResult {
    Ack(RingingV2CommandAck),
    Receipt(qaqh_client::ringing_v2::RingingV2CommandStatus),
    Unknown(String),
}
pub enum PreparedAttachment {
    Image(qaqh_client::ImageBlock),
    Content(ContentRef),
}

/// `shell.list` / `shell.open` 的 wire 镜像(qaqh-runtime service_methods.rs
/// `ShellView` 同形状;gpui 不依赖 runtime crate,故本地镜像)。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShellInfo {
    pub id: String,
    pub pid: u32,
    pub shell: String,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub alive: bool,
    pub output_offset: u64,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ShellListResult {
    shells: Vec<ShellInfo>,
}

/// `skills.catalog` 单个技能条目(镜像 qaqh-runtime `SkillCatalogEntry`)。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
    pub scope: String,
    pub origin: String,
    pub path: String,
}

/// `skills.catalog` 发现期诊断。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkillCatalogDiagnostic {
    pub severity: String,
    pub path: String,
    pub message: String,
}

/// `skills.catalog` daemon 原生返回结构。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SkillsCatalogResult {
    pub skills: Vec<SkillCatalogEntry>,
    pub diagnostics: Vec<SkillCatalogDiagnostic>,
}

/// 附带发现归属工作区名的技能卡片条目。
#[derive(Debug, Clone)]
pub struct ScannedSkill {
    pub entry: SkillCatalogEntry,
    pub workspace: String,
}

#[derive(Debug, Clone)]
pub struct ScannedSkillIssue {
    pub root: String,
    pub message: String,
    pub forbidden: bool,
}

/// 跨工作区去重聚合并排序后的全量 Skills 扫描报告。
#[derive(Debug, Clone, Default)]
pub struct SkillsScanReport {
    pub project: Vec<ScannedSkill>,
    pub user: Vec<ScannedSkill>,
    pub issues: Vec<ScannedSkillIssue>,
    pub daemon_unreachable: bool,
}

/// `provider.presets.list` 官方内置服务商预设视图。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPresetView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub wire: qaqh_types::Wire,
    pub credential: qaqh_types::CredentialSource,
}

/// shell ws 流生命周期给 UI 的镜像(client `ShellStreamStatus` 的 wire 侧投影,
/// electron ShellStreamEvent.ts 同位)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellUiStatus {
    Connecting,
    Open,
    Reconnecting { retry_ms: u64, reason: String },
    /// 本地退订/客户端关闭;shell 进程不受影响(ADR-0028 §D5 断联不 kill)。
    Unsubscribed { reason: String },
    /// 协议 `closed{reason}`:shell 真终止(退出/被收走/daemon 关停)。
    Terminated { reason: String },
}

/// serve 持有的活跃 shell 订阅表:id → 流句柄 + 已交付字节
/// (重连回放的 `from_offset` 权威,ADR-0028 切片 5)。
pub type ShellTable = HashMap<String, ShellEntry>;
pub struct ShellEntry {
    pub stream: ShellStream,
    pub delivered: Arc<AtomicU64>,
}

fn send_line(to_ui: &UnboundedSender<BridgeMsg>, text: &str) {
    let _ = to_ui.unbounded_send(BridgeMsg::Line(text.into()));
}

fn send_shell_error(to_ui: &UnboundedSender<BridgeMsg>, text: &str) {
    send_line(to_ui, text);
    let _ = to_ui.unbounded_send(BridgeMsg::ShellError(text.into()));
}

/// 桥 → GPUI 方向的消息。
pub enum BridgeMsg {
    /// Executor ownership, not a wire envelope: discard callbacks from a closed client.
    FromConnection(u64, Box<BridgeMsg>),
    Connection(ConnectionState),
    Delivery(String, String, DeliveryResult),
    Attachment(String, Result<PreparedAttachment, String>),
    Operation(String, CommandTarget, Result<RingingV2CommandAck, String>),
    /// 人类可读状态行(含命令受理/失败报告)。
    Line(String),
    /// `session.list` 原始数组(前端自行投影 status/workspace_id)。
    Sessions(Vec<SessionListEntry>),
    /// `workspace.list` 条目(侧栏分组)。
    Workspaces(Vec<WorkspaceEntry>),
    /// `skills.catalog` 扫描结果(工作区聚合)。
    SkillsCatalog(SkillsScanReport),
    /// 当前会话的 `todo.list` 快照。
    Todos(String, qaqh_ringing_contract::todo::TodoListSnapshot),
    /// `GET /ringing/v2/sessions/{seed}/team` 快照(团队花名册/任务板/留言板)。
    Team(String, Box<qaqh_client::v2::TeamResponse>),
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
    /// provider.presets.list 的官方预设表。
    ProviderPresets(Vec<ProviderPresetView>),
    /// 取消看门狗到点:该会话仍未回终态(electron CANCEL_WATCHDOG 模式)。
    Watchdog(String),
    /// 服务面结果(siwc/dsaccount 账号管理器):method 原样带回,
    /// UI 按 SDK service_types 反序列化,桥侧零转换。
    ServiceResult {
        method: String,
        result: Result<serde_json::Value, String>,
    },
    SessionSettings {
        seed: String,
        settings: Option<SessionListEntry>,
        write_completed: bool,
    },
    /// `shell.list` 全量(权威;开/关后桥侧顺带刷新)。
    Shells(Vec<ShellInfo>),
    /// `shell.open` 单条结果(UI 追加并聚焦)。
    ShellOpened(ShellInfo),
    /// `shell.close` 成功(UI 移除标签)。
    ShellRemoved(String),
    /// shell ws 下行裸字节(PTY 输出)。
    ShellBytes { shell_id: String, bytes: Vec<u8> },
    /// shell ws 生命周期状态迁移。
    ShellStatus { shell_id: String, status: ShellUiStatus },
    /// shell 操作/订阅失败，由终端坞与全局提示直接展示。
    ShellError(String),
}

/// 测试可观测性:`QAQH_GPUI_TRACE=1` 时把协议层动态打到 stderr。
fn trace(msg: &str) {
    if std::env::var_os("QAQH_GPUI_TRACE").is_some() {
        eprintln!("[qaqh-gpui] {msg}");
    }
}

/// typed 查询:QueryRequest 封闭枚举由客户端映射 method+params,前端不再裸写方法名。
async fn query_typed<T: serde::de::DeserializeOwned>(
    client: &Client,
    request: QueryRequest,
) -> Result<T, qaqh_client::ClientError> {
    let value = client.query(request).await?;
    serde_json::from_value(value).map_err(qaqh_client::ClientError::from)
}

/// GPUI → 桥方向的请求。
pub enum BridgeRequest {
    Reconnect,
    Command { target: CommandTarget, command: RingingCommand },
    Receipt { seed: String, command_id: String },
    AttachFile { seed: String, path: std::path::PathBuf },
    ReviewContent {
        session_id: String,
        turn: TurnId,
        request: usize,
        references: Vec<ContentRef>,
    },
    /// attach(session.resume)+ activate_stream + bootstrap 快照。
    Select(String),
    /// 立即刷新侧栏会话列表(交接建行后加速锚点定位)。
    RefreshRoster,
    /// 重新 bootstrap 并投递快照(reset 后)。
    Refill(String),
    /// conversation.send_message。
    SendMessage { seed: String, outbox: crate::delivery::Outbox },
    /// tool.permission.respond。
    RespondPermission {
        seed: String,
        tool_call_id: String,
        approved: bool,
        /// 本次/本会话/持久信任作用范围；拒绝时为 None。
        scope: Option<PermissionGrantScope>,
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
    /// team 快照查询(当前会话;无推送,选中/轮询触发)。
    QueryTeam { seed: String },
    /// conversation.cancel。
    Cancel {
        seed: String,
        turn_id: Option<String>,
    },
    /// session.create(cwd = 目标工作区路径)。
    /// `first_message` 给 handoff fork:创建成功后立即投递交接文档(trigger_turn)。
    CreateSession {
        cwd: Option<String>,
        model_selection: Option<ModelSelection>,
        tool_mode: Option<String>,
        title: Option<String>,
        first_message: Option<String>,
        /// handoff 工作链接缝(ADR-0015 切片F):来源会话 id;普通新建缺席。
        handoff_from: Option<String>,
    },
    PatchSessionSettings {
        seed: String,
        patch: SessionSettingsPatch,
        revision: u64,
    },
    /// workspace.move_session(把该会话移入目标分组)。
    MoveSession { seed: String, workspace_id: String },
    SaveConfig(ConfigPatch),
    ManageCatalog {
        command: RingingCommand,
        revision: u64,
    },
    /// provider.list(命令;结果在 ack.result)。
    ListProviders,
    /// provider.presets.list(命令;结果在 ack.result)。
    ListProviderPresets,
    /// config.load 查询。
    QueryConfig,
    /// `skills.catalog` 扫描请求:传 `(Option<cwd>, 显示工作区名)` 列表。
    QuerySkills {
        workspaces: Vec<(Option<String>, String)>,
    },
    /// session.delete(名册轮询自然刷新列表)。
    DeleteSession { seed: String },
    /// 取消看门狗:after_ms 后仍无终态则通知 UI 本地收口。
    Watchdog { seed: String, after_ms: u64 },
    /// 服务面透传(siwc/dsaccount);method 须在 SDK SERVICE_METHODS 白名单,
    /// 越界由 SDK 客户端先拒。
    Service {
        method: String,
        params: serde_json::Value,
    },
    /// `shell.list`(终端坞打开/重连后刷新)。
    ShellList,
    /// `shell.open`(cols/rows 由桥侧按 ansi 解析器口径固定 80×24)。
    ShellOpen { cwd: Option<String>, shell: Option<String> },
    /// `shell.close`(真收进程;UI 先行移除标签)。
    ShellClose { shell_id: String },
    /// ws `stdin{bytes}`;一次 keystroke / 一次粘贴各一帧。
    ShellStdin { shell_id: String, bytes: Vec<u8> },
    /// ws `resize{cols,rows}`(无回执;真值读 shell.list)。
    ShellResize { shell_id: String, cols: u16, rows: u16 },
    /// 主动重建订阅:回放整段有界尾,配合 UI 清屏。
    ShellAttach { shell_id: String },
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
        launch_daemon_if_missing: true,
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

    let Some(open) = client.v2_session_state().await else {
        report("连接失败:RingingV2 握手未完成，daemon 版本可能过旧".into());
        return;
    };
    report(format!(
        "v2 open:client_session={},epoch={},commands={},session={}",
        open.client_session_id,
        open.server_epoch,
        qaqh_client::ringing::COMMAND_CONTRACT_REVISION,
        qaqh_client::ringing::SESSION_CONTRACT_REVISION,
    ));

    match query_typed::<Vec<SessionListEntry>>(&client, QueryRequest::SessionList).await {
        Ok(value) => {
            let count = value.len();
            report(format!("session.list:{count} 个会话"));
            for entry in value.iter().take(8) {
                report(format!(
                    "  · {} 「{}」",
                    entry.meta.session_id,
                    entry.meta.title.as_deref().unwrap_or("(无标题)"),
                ));
            }
        }
        Err(error) => report(format!("session.list 失败:{error}")),
    }
}

/// 桥线程主循环:复用 qaqh-client 的全局 tokio 运行时,挂四个回调,服务请求。
pub fn run(to_ui: UnboundedSender<BridgeMsg>, requests: UnboundedReceiver<BridgeRequest>, client_instance_id: String) {
    qaqh_client::runtime_handle().block_on(serve(to_ui, requests, client_instance_id));
}

async fn serve(
    to_ui: UnboundedSender<BridgeMsg>,
    mut requests: UnboundedReceiver<BridgeRequest>,
    client_instance_id: String,
) {
    let mut client: Option<Client> = None;
    let mut selected: Option<String> = None;
    let mut failures = 0;
    let mut shells: ShellTable = HashMap::new();
    let mut roster = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            request = requests.next() => {
                let Some(request) = request else { break };
                if matches!(request, BridgeRequest::Reconnect) {
                    if let Some(previous) = client.take() { previous.close(); }
                    let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Connecting));
                    roster.reset_immediately();
                    continue;
                }
                if let BridgeRequest::Select(seed) = &request { selected = Some(seed.clone()); }
                if let Some(client) = &client {
                    handle_request(client, &to_ui, &mut shells, request).await;
                } else {
                    match request {
                        BridgeRequest::SendMessage { seed, outbox } => {
                            let _ = to_ui.unbounded_send(BridgeMsg::Delivery(seed, outbox.command_id, DeliveryResult::Unknown("尚未连接；原请求已保存".into())));
                        }
                        BridgeRequest::Select(_) | BridgeRequest::RefreshRoster => {},
                        BridgeRequest::AttachFile { seed, .. } => {
                            let _ = to_ui.unbounded_send(BridgeMsg::Attachment(seed, Err("连接尚未恢复".into())));
                        }
                        BridgeRequest::PatchSessionSettings { seed, .. } => {
                            let _ = to_ui.unbounded_send(BridgeMsg::SessionSettings { seed, settings: None, write_completed: true });
                        }
                        BridgeRequest::CreateSession { .. } => { let _ = to_ui.unbounded_send(BridgeMsg::CreateFailed); }
                        BridgeRequest::QuerySkills { .. } => {
                            let _ = to_ui.unbounded_send(BridgeMsg::SkillsCatalog(SkillsScanReport {
                                daemon_unreachable: true,
                                ..Default::default()
                            }));
                        }
                        BridgeRequest::Service { method, .. } => {
                            let _ = to_ui.unbounded_send(BridgeMsg::ServiceResult { method, result: Err("连接尚未恢复".into()) });
                        }
                        BridgeRequest::ShellList => send_line(&to_ui, "连接尚未恢复，跳过 shell.list"),
                        BridgeRequest::ShellOpen { .. }
                        | BridgeRequest::ShellClose { .. }
                        | BridgeRequest::ShellStdin { .. }
                        | BridgeRequest::ShellResize { .. }
                        | BridgeRequest::ShellAttach { .. } => {
                            send_shell_error(&to_ui, "daemon 尚未连接，无法打开或操作交互终端");
                        }
                        _ => send_line(&to_ui, "连接尚未恢复，请连接成功后重试操作"),
                    }
                }
            }
            _ = roster.tick() => {
                if let Some(active) = &client {
                    if poll_roster(active, &to_ui).await { failures = 0; } else { failures += 1; }
                    if failures >= 3 {
                        active.close(); client = None;
                        let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Failed { detail: "daemon 查询连续失败，正在重新发现服务".into() }));
                    }
                } else {
                    let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Connecting));
                    match establish(&to_ui, &client_instance_id).await {
                        Ok(active) => {
                            failures = 0;
                            query_providers(&active, &to_ui).await;
                            poll_roster(&active, &to_ui).await;
                            if let Some(seed) = &selected {
                                handle_request(&active, &to_ui, &mut shells, BridgeRequest::Select(seed.clone())).await;
                            }
                            // 换了新 client,旧订阅随 client_stop 已死;按既有
                            // delivered 重订阅 —— 回放从断点续,不重刷屏。
                            for shell_id in shells.keys().cloned().collect::<Vec<_>>() {
                                if let Err(error) = shell_spawn(&active, &to_ui, &mut shells, &shell_id).await {
                                    send_shell_error(&to_ui, &format!("shell 重订阅失败({shell_id}):{error}"));
                                }
                            }
                            client = Some(active);
                        }
                        Err(error) => {
                            crate::diagnostics::record("connect_failed", None, None, None);
                            let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Failed { detail: error }));
                        }
                    }
                }
            },
        }
    }
    if let Some(client) = client { client.close(); }
}

/// 完整建立:discovery(缺失则尝试拉起)→ connect → open_v2 → session.list。
async fn establish(
    to_ui: &UnboundedSender<BridgeMsg>,
    client_instance_id: &str,
) -> Result<Client, String> {
    let generation = CONNECTION_GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    let event_tx = to_ui.clone();
    let handlers = ClientHandlers {
        on_v2_event: Arc::new(move |seed: String, frame: SessionEventFrame| {
            trace(&format!("event seed={seed} seq={}", frame.seq));
            from_connection(&event_tx, generation, BridgeMsg::Event(seed, frame));
        }),
        on_v2_reset: {
            let tx = to_ui.clone();
            Arc::new(move |seed: String, reset: RingingV2ResyncRequired| {
                trace(&format!("reset seed={seed} {reset:?}"));
                from_connection(&tx, generation, BridgeMsg::Reset(seed, reset));
            })
        },
        on_v2_status: {
            let tx = to_ui.clone();
            Arc::new(move |seed: String, status: V2StreamStatus| {
                trace(&format!("stream seed={seed} {status:?}"));
                from_connection(&tx, generation, BridgeMsg::Stream(seed, status));
            })
        },
        on_liveness: {
            // 字节层存活仅留协议 trace:UI 侧无消费方,不发 BridgeMsg。
            Arc::new(|| trace("liveness 字节到达"))
        },
    };

    let client = Client::connect_async(ClientOptions {
        handlers,
        client_instance_id: client_instance_id.to_string(),
        launch_daemon_if_missing: true,
        ..ClientOptions::default()
    })
    .await
    .map_err(|error| error.to_string())?;
    send_line(to_ui, "client 已建立");

    // 握手失败不能 panic:桥线程一死 UI 将无提示僵死,这里上报后走重连节奏。
    let Some(open) = client.v2_session_state().await else {
        client.close();
        let detail = "RingingV2 握手未完成，daemon 版本可能过旧".to_string();
        send_line(to_ui, &format!("连接失败:{detail}"));
        return Err(detail);
    };
    crate::diagnostics::set_epoch(open.server_epoch.clone());
    let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Ready { client_session_id: open.client_session_id.clone() }));
    crate::diagnostics::record("connected", None, None, None);
    send_line(
        to_ui,
        &format!(
            "v2 open:client_session={},epoch={},commands={},session={}",
            open.client_session_id,
            open.server_epoch,
            qaqh_client::ringing::COMMAND_CONTRACT_REVISION,
            qaqh_client::ringing::SESSION_CONTRACT_REVISION,
        ),
    );
    trace(&format!(
        "open csid={} epoch={} commands={} session={}",
        open.client_session_id,
        open.server_epoch,
        qaqh_client::ringing::COMMAND_CONTRACT_REVISION,
        qaqh_client::ringing::SESSION_CONTRACT_REVISION,
    ));

    match client
        .service_v2::<_, Vec<SessionListEntry>>("session.list", json!({ "include_archived": true }))
        .await
    {
        Ok(value) => {
            let count = value.len();
            trace(&format!("session.list {count} 条"));
            // 原始数组直传:前端投影需要 status/workspace_id(sanitize 会丢)。
            // 例行结果不再 send_line 进 UI——那曾是侧栏底部日志泄漏的源头。
            let _ = to_ui.unbounded_send(BridgeMsg::Sessions(value));
        }
        Err(error) => send_line(to_ui, &format!("session.list 失败:{error}")),
    }
    Ok(client)
}

async fn handle_request(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    shells: &mut ShellTable,
    request: BridgeRequest,
) {
    match request {
        BridgeRequest::Reconnect => unreachable!("handled by connection owner"),
        BridgeRequest::Command { target, command } => {
            send_command(client, to_ui, target.clone(), command).await;
            if let CommandTarget::Session { session_id } = target { query_session_settings(client, to_ui, &session_id, false).await; }
            poll_roster(client, to_ui).await;
        }
        BridgeRequest::Receipt { seed, command_id } => {
            let result = match client.command_status_v2_typed(&command_id).await {
                Ok(status) => DeliveryResult::Receipt(status),
                Err(error) => DeliveryResult::Unknown(format!("回执查询失败：{error}")),
            };
            let _ = to_ui.unbounded_send(BridgeMsg::Delivery(seed, command_id, result));
        }
        BridgeRequest::AttachFile { seed, path } => {
            let client = client.clone(); let tx = to_ui.clone();
            tokio::spawn(async move {
                let result = prepare_attachment(&client, &seed, &path).await;
                let _ = tx.unbounded_send(BridgeMsg::Attachment(seed, result));
            });
        }
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
            for previous in client.active_streams().await {
                if previous != seed { client.deactivate_stream(&previous).await; }
            }
            trace(&format!("select {seed}:attach+activate+bootstrap"));
            if let Err(error) = client.attach(&seed).await {
                trace(&format!("attach {seed} 失败:{error}"));
                send_line(to_ui, &format!("attach {seed} 失败:{error}"));
                return;
            }
            client.activate_stream(&seed).await;
            refill(client, to_ui, &seed).await;
            query_todos(client, to_ui, &seed).await;
            query_team(client, to_ui, &seed).await;
        }
        BridgeRequest::RefreshRoster => {
            poll_roster(client, to_ui).await;
        }
        BridgeRequest::Refill(seed) => refill(client, to_ui, &seed).await,
        BridgeRequest::SendMessage { seed, outbox } => {
            let id = outbox.command_id;
            crate::diagnostics::record("message_submit", Some(&seed), Some(&id), None);
            let result = match client.send_command_v2_typed(
                CommandTarget::Session { session_id: seed.clone() },
                RingingCommand::ConversationSendMessage(outbox.message),
                CommandOptions { command_id: Some(id.clone()), ..Default::default() },
            ).await {
                Ok(ack) => DeliveryResult::Ack(ack),
                Err(error) => DeliveryResult::Unknown(error.to_string()),
            };
            let _ = to_ui.unbounded_send(BridgeMsg::Delivery(seed.clone(), id.clone(), result));
            if let Ok(status) = client.command_status_v2_typed(&id).await {
                let _ = to_ui.unbounded_send(BridgeMsg::Delivery(seed, id, DeliveryResult::Receipt(status)));
            }
        }
        BridgeRequest::RespondPermission {
            seed,
            tool_call_id,
            approved,
            scope,
        } => {
            let trust_folder = approved && matches!(scope, Some(PermissionGrantScope::Persistent));
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::ToolPermissionRespond {
                    tool_call_id,
                    approved,
                    trust_folder,
                    scope: approved.then_some(scope).flatten(),
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
        BridgeRequest::QueryTeam { seed } => query_team(client, to_ui, &seed).await,
        BridgeRequest::Cancel { seed, turn_id } => {
            send_command(
                client,
                to_ui,
                CommandTarget::Session { session_id: seed },
                RingingCommand::ConversationCancel { turn_id },
            )
            .await;
        }
        BridgeRequest::CreateSession { cwd, model_selection, tool_mode, title, first_message, handoff_from } => {
            let command = RingingCommand::SessionCreate {
                cwd: cwd.clone(),
                title,
                model_selection,
                tool_mode,
                custom_tools: None,
                handoff_from,
            };
            let result = send_command(client, to_ui, CommandTarget::Global, command)
                .await
                .and_then(|ack| ack.result)
                .and_then(|result| match serde_json::from_value::<qaqh_client::SessionCreateResult>(result) {
                    Ok(result) => Some(result.session_id),
                    Err(error) => { send_line(to_ui, &format!("创建结果不符合契约：{error}")); None }
                });
            if let (Some(seed), Some(message)) = (result.clone(), first_message) {
                // handoff fork:文档投给 child(input_purpose 缺省 trigger_turn)。
                let command = RingingCommand::ConversationSendMessage(qaqh_client::ConversationSendMessage {
                    text: message,
                    images: Vec::new(),
                    attachments: None,
                    message_id: None,
                    input_purpose: Default::default(),
                    as_system: false,
                    inter_agent: None,
                    subagent_terminal: None,
                    widget_context: None,
                });
                send_command(client, to_ui, CommandTarget::Session { session_id: seed }, command).await;
            }
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
            let _ = to_ui.unbounded_send(BridgeMsg::Operation("session.settings.patch".into(), CommandTarget::Session { session_id: seed.clone() }, result.as_ref().cloned().map_err(ToString::to_string)));
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
        BridgeRequest::SaveConfig(patch) => {
            match serde_json::to_value(patch) {
                Ok(patch) => {
                    let result = client.send_command_v2_typed(CommandTarget::Global, RingingCommand::ConfigSave { patch }, CommandOptions::default()).await;
                    let _ = to_ui.unbounded_send(BridgeMsg::Operation("config.save".into(), CommandTarget::Global, result.map_err(|error| error.to_string())));
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
            let result = client.send_command_v2_typed(CommandTarget::Global, command, options).await;
            let _ = to_ui.unbounded_send(BridgeMsg::Operation(name.into(), CommandTarget::Global, result.as_ref().cloned().map_err(ToString::to_string)));
            match result {
                Ok(ack) => send_line(to_ui, &format!("{name}: {:?} {:?}", ack.status, ack.code)),
                Err(error) => send_line(to_ui, &format!("{name} 失败: {error}")),
            }
            query_providers(client, to_ui).await;
            query_config(client, to_ui).await;
        }
        BridgeRequest::ListProviders => query_providers(client, to_ui).await,
        BridgeRequest::ListProviderPresets => query_provider_presets(client, to_ui).await,

        BridgeRequest::QueryConfig => query_config(client, to_ui).await,
        BridgeRequest::QuerySkills { workspaces } => query_skills(client, to_ui, workspaces).await,
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
        BridgeRequest::Service { method, params } => {
            let result = client.service_call(&method, params).await.map_err(|error| error.to_string());
            let _ = to_ui.unbounded_send(BridgeMsg::ServiceResult { method, result });
        }
        BridgeRequest::ShellList => match client
            .service_v2::<_, ShellListResult>("shell.list", json!({}))
            .await
        {
            Ok(result) => {
                shells.retain(|id, _| result.shells.iter().any(|info| &info.id == id));
                let to_subscribe: Vec<String> = result
                    .shells
                    .iter()
                    .filter(|info| info.alive && !shells.contains_key(&info.id))
                    .map(|info| info.id.clone())
                    .collect();
                let _ = to_ui.unbounded_send(BridgeMsg::Shells(result.shells));
                for shell_id in to_subscribe {
                    if let Err(error) = shell_spawn(client, to_ui, shells, &shell_id).await {
                        send_shell_error(to_ui, &format!("shell 流订阅失败({shell_id}):{error}"));
                    }
                }
            }
            Err(error) => send_shell_error(to_ui, &format!("shell.list 失败:{error}")),
        },
        BridgeRequest::ShellOpen { cwd, shell } => {
            // 几何按 ansi.rs 解析器口径固定(终端坞不做 per-tab 测量)。
            let params = json!({ "cols": 80u16, "rows": 24u16, "cwd": cwd, "shell": shell });
            match client.service_v2::<_, ShellInfo>("shell.open", params).await {
                Ok(info) => {
                    let shell_id = info.id.clone();
                    // 先把页签发给 UI,再建立 ws 流,避免首帧状态/字节早于建页签到达。
                    let _ = to_ui.unbounded_send(BridgeMsg::ShellOpened(info));
                    if let Err(error) = shell_spawn(client, to_ui, shells, &shell_id).await {
                        send_shell_error(to_ui, &format!("shell 流订阅失败:{error}"));
                    }
                }
                Err(error) => send_shell_error(to_ui, &format!("shell.open 失败:{error}")),
            }
        }
        BridgeRequest::ShellClose { shell_id } => {
            // 先退订阅再收进程:close 只断 ws 订阅,服务面才 kill(§D5)。
            shells.remove(&shell_id);
            match client
                .service_v2::<_, serde_json::Value>("shell.close", json!({ "shell_id": shell_id }))
                .await
            {
                Ok(_) => {
                    let _ = to_ui.unbounded_send(BridgeMsg::ShellRemoved(shell_id));
                }
                Err(error) => send_shell_error(to_ui, &format!("shell.close 失败:{error}")),
            }
        }
        BridgeRequest::ShellStdin { shell_id, bytes } => {
            if let Some(entry) = shells.get(&shell_id) {
                if let Err(error) = entry.stream.stdin(&bytes).await {
                    send_shell_error(to_ui, &format!("shell 输入丢失({shell_id}):{error}"));
                }
            } else {
                send_shell_error(
                    to_ui,
                    &format!("shell 数据流未订阅({shell_id})，可点击「清屏重建」重连"),
                );
            }
        }
        BridgeRequest::ShellResize { shell_id, cols, rows } => {
            if let Some(entry) = shells.get(&shell_id) {
                let _ = entry.stream.resize(cols, rows).await;
            }
        }
        BridgeRequest::ShellAttach { shell_id } => {
            if let Some(entry) = shells.get_mut(&shell_id) {
                entry.delivered.store(0, Ordering::Relaxed);
                if let Err(error) = entry.stream.attach(None).await {
                    send_shell_error(to_ui, &format!("shell 重放请求失败({shell_id}):{error}"));
                }
            } else if let Err(error) = shell_spawn(client, to_ui, shells, &shell_id).await {
                send_shell_error(to_ui, &format!("shell 重订阅失败({shell_id}):{error}"));
            }
        }
    }
}

/// 起一条 shell ws 订阅:回调把字节/状态镜像进 BridgeMsg;delivered 计数
/// 跨重连保留在表里,Worker 重拨自动带 `from_offset`(shell_stream.rs §D5)。
async fn shell_spawn(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    shells: &mut ShellTable,
    shell_id: &str,
) -> Result<(), String> {
    let generation = CONNECTION_GENERATION.load(Ordering::Acquire);
    // delivered 跨重连保留:重订阅时复用旧条目的计数,回放从断点续。
    let delivered: Arc<AtomicU64> = shells
        .get(shell_id)
        .map(|entry| entry.delivered.clone())
        .unwrap_or_else(|| Arc::new(AtomicU64::new(0)));
    let bytes_tx = to_ui.clone();
    let bytes_delivered = delivered.clone();
    let event_tx = to_ui.clone();
    let status_tx = to_ui.clone();
    let handlers = ShellStreamHandlers {
        on_bytes: Arc::new(move |shell_id: String, bytes: Vec<u8>| {
            bytes_delivered.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            from_connection(&bytes_tx, generation, BridgeMsg::ShellBytes { shell_id, bytes });
        }),
        on_event: Arc::new(move |shell_id: String, event: ShellStreamEvent| {
            // 词表只有 closed 一帧(§D3),直接解构。
            let ShellStreamEvent::Closed { reason } = event;
            from_connection(&event_tx, generation, BridgeMsg::ShellStatus {
                shell_id,
                status: ShellUiStatus::Terminated { reason },
            });
        }),
        on_status: Arc::new(move |shell_id: String, status: ShellStreamStatus| {
            let status = match status {
                ShellStreamStatus::Connecting => ShellUiStatus::Connecting,
                ShellStreamStatus::Open { .. } => ShellUiStatus::Open,
                ShellStreamStatus::Reconnecting { retry_ms, reason, .. } => {
                    ShellUiStatus::Reconnecting { retry_ms, reason }
                }
                ShellStreamStatus::Closed { reason } => ShellUiStatus::Unsubscribed { reason },
            };
            from_connection(&status_tx, generation, BridgeMsg::ShellStatus { shell_id, status });
        }),
    };
    let stream = ShellStream::start(shell_id, client, handlers)
        .await
        .map_err(|error| error.to_string())?;
    shells.insert(shell_id.to_string(), ShellEntry { stream, delivered });
    Ok(())
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

async fn query_provider_presets(client: &Client, to_ui: &UnboundedSender<BridgeMsg>) {
    let result = client
        .send_command_v2_typed(
            CommandTarget::Global,
            RingingCommand::ProviderPresets {},
            CommandOptions::default(),
        )
        .await;
    let result = match result {
        Ok(ack) => match ack.result {
            Some(result) => result,
            None => {
                send_line(
                    to_ui,
                    &format!("服务商预设读取失败: {:?} {:?}", ack.status, ack.code),
                );
                return;
            }
        },
        Err(error) => {
            send_line(to_ui, &format!("服务商预设读取失败: {error}"));
            return;
        }
    };
    match serde_json::from_value(result) {
        Ok(presets) => {
            let _ = to_ui.unbounded_send(BridgeMsg::ProviderPresets(presets));
        }
        Err(error) => send_line(to_ui, &format!("服务商预设解析失败: {error}")),
    }
}

async fn query_config(client: &Client, to_ui: &UnboundedSender<BridgeMsg>) {
    match query_typed::<ConfigDto>(client, QueryRequest::ConfigLoad).await {
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
    let meta = match query_typed::<Option<SessionListEntry>>(
        client,
        QueryRequest::SessionMeta { session_id: seed.to_owned() },
    )
    .await
    {
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

/// 名册轮询。session.list 需带 include_archived,而 QueryRequest::SessionList
/// 尚无参数位——保留通用出口,后端建模变体后收编 query_typed。
async fn poll_roster(client: &Client, to_ui: &UnboundedSender<BridgeMsg>) -> bool {
    let success = match client
        .service_v2::<_, Vec<SessionListEntry>>("session.list", json!({ "include_archived": true }))
        .await
    {
        Ok(value) => {
            trace(&format!("session.list {}", value.len()));
            let _ = to_ui.unbounded_send(BridgeMsg::Sessions(value));
            true
        }
        Err(error) => { trace(&format!("session.list 失败:{error}")); false },
    };
    if let Some(state) = client.v2_session_state().await {
        crate::diagnostics::set_epoch(state.server_epoch);
        let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Ready { client_session_id: state.client_session_id }));
    }
    match query_typed::<Vec<WorkspaceEntry>>(client, QueryRequest::WorkspaceList).await {
        Ok(entries) => {
            trace(&format!("workspace.list {} 组", entries.len()));
            let _ = to_ui.unbounded_send(BridgeMsg::Workspaces(entries));
        }
        Err(error) => trace(&format!("workspace.list 失败:{error}")),
    }
    success
}

/// todo.list 查询;失败只 trace,UI 保留旧快照。
async fn query_todos(client: &Client, to_ui: &UnboundedSender<BridgeMsg>, seed: &str) {
    match client
        .service_v2::<_, qaqh_ringing_contract::todo::TodoListSnapshot>("todo.list", json!({ "session_id": seed }))
        .await
    {
        Ok(value) => {
            let _ = to_ui.unbounded_send(BridgeMsg::Todos(seed.to_string(), value));
        }
        Err(error) => trace(&format!("todo.list {seed} 失败:{error}")),
    }
}

/// team 快照查询;失败只 trace(无 TeamDelta 推送,靠选中/轮询刷新)。
async fn query_team(client: &Client, to_ui: &UnboundedSender<BridgeMsg>, seed: &str) {
    match client.team_v2(seed).await {
        Ok(value) => {
            trace(&format!("team {seed} tasks={}", value.tasks.tasks.len()));
            let _ = to_ui.unbounded_send(BridgeMsg::Team(seed.to_string(), Box::new(value)));
        }
        Err(error) => trace(&format!("team {seed} 失败:{error}")),
    }
}

/// 按工作区聚合查询 daemon `skills.catalog`,按 path 去重并按 name 排序。
async fn query_skills(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    workspaces: Vec<(Option<String>, String)>,
) {
    let scopes = if workspaces.is_empty() {
        vec![(None, "当前目录".to_owned())]
    } else {
        workspaces
    };
    let mut project = Vec::new();
    let mut user = Vec::new();
    let mut issues = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();
    let mut any_succeeded = false;

    for (cwd, label) in scopes {
        match query_typed::<SkillsCatalogResult>(client, QueryRequest::SkillsCatalog { cwd }).await {
            Ok(result) => {
                any_succeeded = true;
                for entry in result.skills {
                    if !seen_paths.insert(entry.path.clone()) {
                        continue;
                    }
                    let is_user = entry.scope == "user";
                    let scanned = ScannedSkill {
                        entry,
                        workspace: label.clone(),
                    };
                    if is_user {
                        user.push(scanned);
                    } else {
                        project.push(scanned);
                    }
                }
                for diagnostic in result.diagnostics {
                    let root = if diagnostic.path.is_empty() {
                        label.clone()
                    } else {
                        diagnostic.path
                    };
                    issues.push(ScannedSkillIssue {
                        root,
                        message: format!("{}: {}", diagnostic.severity, diagnostic.message),
                        forbidden: false,
                    });
                }
            }
            Err(error) => {
                let msg = error.to_string();
                let lower = msg.to_ascii_lowercase();
                let forbidden = lower.contains("forbidden") || lower.contains("not allowed");
                issues.push(ScannedSkillIssue {
                    root: label,
                    message: msg,
                    forbidden,
                });
            }
        }
    }

    project.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    user.sort_by(|a, b| a.entry.name.cmp(&b.entry.name));
    let _ = to_ui.unbounded_send(BridgeMsg::SkillsCatalog(SkillsScanReport {
        project,
        user,
        issues,
        daemon_unreachable: !any_succeeded,
    }));
}

async fn prepare_attachment(client: &Client, seed: &str, path: &std::path::Path) -> Result<PreparedAttachment, String> {
    use base64::Engine;
    use std::io::Read;
    let path = path.to_owned();
    let (bytes, media) = tokio::task::spawn_blocking(move || -> Result<_, String> {
        let mut bytes = Vec::new();
        std::fs::File::open(&path).map_err(|e| e.to_string())?.take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        if bytes.len() > 4 * 1024 * 1024 { return Err("附件最大 4 MiB".into()); }
        let media = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") { "image/png" }
            else if bytes.starts_with(&[0xff, 0xd8, 0xff]) { "image/jpeg" }
            else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") { "image/gif" }
            else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") { "image/webp" }
            else if std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0) { "text/plain; charset=utf-8" }
            else { return Err("支持 PNG、JPEG、GIF、WebP 图片和 UTF-8 文本附件".into()); };
        Ok((bytes, media))
    }).await.map_err(|e| e.to_string())??;
    if media.starts_with("image/") {
        Ok(PreparedAttachment::Image(qaqh_client::ImageBlock { mime_type: media.into(), data: base64::engine::general_purpose::STANDARD.encode(bytes) }))
    } else {
        client.upload_content(seed, media, bytes).await.map(PreparedAttachment::Content).map_err(|e| e.to_string())
    }
}

/// 命令直发;ack/错误如实上报,ack 交回调用方(result/revision 载荷)。
async fn send_command(
    client: &Client,
    to_ui: &UnboundedSender<BridgeMsg>,
    target: CommandTarget,
    command: RingingCommand,
) -> Option<RingingV2CommandAck> {
    let operation = command.name().to_string();
    let outcome = client.send_command_v2_typed(target.clone(), command, CommandOptions::default()).await;
    let _ = to_ui.unbounded_send(BridgeMsg::Operation(operation, target, outcome.as_ref().map(Clone::clone).map_err(ToString::to_string)));
    match outcome {
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
    // timeline_tail=None = 全量快照(旧语义);Some(n) 需 UI 配合 timeline_from_ord
    // 按需取历史,reducer 尚未接线,不半途启用。
    match client.bootstrap_v2(seed, None).await {
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
