//! tokio ↔ gpui 桥（RingingV3：本地 IPC 单连接 + 四类逻辑流）。
//!
//! ## 这一层只做四件事
//!
//! 1. **连接**：`daemon.endpoint` → `V3Client::connect_local` → 握手；断联即重连（显式动作，不静默续命）；
//! 2. **四条流**：控制面（一问一答）/ 会话事件（每选中会话一条）/ Shell PTY（每 tab 一条）/ Blob（收件箱一条）；
//! 3. **域请求直通**：UI 直接构造 `qaqh_ringing_v3` 的强类型请求（零前端 DTO、零方法名字符串）；
//! 4. **本地副作用**：名册轮询、快照/翻页、附件上传、定时看门狗。
//!
//! 桥**不翻译业务语义**：`BridgeMsg` 载荷一律是 V3 强类型，渲染口径由 `qaqh-gpui-session` 决定。
//!
//! 模块划分：本文件 = 消息面 + 连接主循环；`streams` = 三条流任务；`requests` = 请求派发；
//! `attach` = 附件与引用内容。

mod attach;
mod requests;
mod streams;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::StreamExt;
use qaqh_client::v3::V3Client;
use qaqh_core_vocab::{ContentHash, ContentRef, SessionId, TurnId};
use qaqh_ringing_v3::{
    AccountRequest, AccountResponse, CommandRequest, CommandResponse, ConfigRequest, ConfigResponse,
    ControlRequest, ControlResponse, SessionEventFrame, SessionRequest, SessionResponse,
    SessionSnapshot, SessionSummary, ShellRequest, ShellResponse, ShellView, ToolCatalogRequest,
    ToolCatalogResponse, TranscriptPage, WorkspaceEntry, WorkspaceRequest, WorkspaceResponse,
};
use qaqh_v3_transport::YamuxTransport;

use requests::handle_request;
use streams::{attach_events, spawn_blob_inbox};

/// 首屏尾部窗口（块）；`v3_bridge` 上界 500，超出走 `LoadTranscript` 向上翻页。
pub const TAIL_BLOCKS: u32 = 500;
/// 首屏尾部窗口（回合）。
pub const TAIL_TURNS: u32 = 60;
/// 名册轮询周期。
const ROSTER_PERIOD: Duration = Duration::from_secs(5);

static CONNECTION_GENERATION: AtomicU64 = AtomicU64::new(0);

/// 当前连接代际（旧连接的流任务回调据此丢弃）。
pub fn connection_generation() -> u64 {
    CONNECTION_GENERATION.load(Ordering::Acquire)
}

fn next_generation() -> u64 {
    CONNECTION_GENERATION.fetch_add(1, Ordering::AcqRel) + 1
}

fn from_connection(sender: &UnboundedSender<BridgeMsg>, generation: u64, message: BridgeMsg) {
    let _ = sender.unbounded_send(BridgeMsg::FromConnection(generation, Box::new(message)));
}

/// 连接状态（UI 徽标）。
#[derive(Clone)]
pub enum ConnectionState {
    Connecting,
    Ready { server_instance_id: String, build_id: String },
    Failed { detail: String },
}

/// 单条命令的投递结果（V3 没有异步回执查询：受理 + 事件流就是权威）。
pub enum DeliveryResult {
    Accepted { command_id: String },
    Rejected(String),
    Unknown(String),
}

/// 附件准备结果：图片与文本统一走内容寻址 blob，**不再 base64 内联**。
pub struct PreparedAttachment {
    pub hash: ContentHash,
    pub len: u64,
    pub mime: String,
}

/// Shell 流生命周期（UI 终端坞标签）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellUiStatus {
    Connecting,
    Open,
    /// 本地退订/关闭；进程不受影响（ADR-0028 §D5 断联不 kill）。
    Unsubscribed { reason: String },
    /// 服务端宣告 PTY 结束。
    Terminated { reason: String },
}

/// 一条会话事件流的体检（UI 只用来画"流健康"点）。
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamProgress {
    pub frames: u64,
    pub last_seq: u64,
    pub resync_cycles: u64,
    pub ordering_violations: u64,
    /// 该流已终止（服务端断开/错误）；`false` = 仍在订阅。
    pub ended: bool,
}

/// 桥 → GPUI 方向的消息（载荷全是 V3 强类型）。
pub enum BridgeMsg {
    /// Executor ownership：旧连接的回调一律丢弃。
    FromConnection(u64, Box<BridgeMsg>),
    Connection(ConnectionState),
    /// 人类可读状态行（命令受理/失败报告）。
    Line(String),
    /// 名册（`session.list`）。
    Sessions(Vec<SessionSummary>),
    /// 工作区分组（`workspace.list`）。
    Workspaces(Vec<WorkspaceEntry>),
    /// 首屏/重灌快照（尾部窗口）。
    Snapshot(Box<SessionSnapshot>),
    /// 向上翻页结果。
    Transcript(Box<TranscriptPage>),
    /// 会话事件帧（**前端状态唯一真相源**）。
    Event(SessionId, SessionEventFrame),
    /// 单流体检迁移。
    Stream(SessionId, StreamProgress),
    /// 会话域回执（UI 自己构造请求，自己解释回执）。
    Session(Result<SessionResponse, String>),
    /// 命令域回执。
    Command {
        session_id: Option<SessionId>,
        command_id: String,
        result: Result<CommandResponse, String>,
    },
    Config(Result<ConfigResponse, String>),
    Account(Result<AccountResponse, String>),
    Workspace(Result<WorkspaceResponse, String>),
    ToolCatalog {
        session_id: SessionId,
        result: Result<ToolCatalogResponse, String>,
    },
    /// Shell 域回执（write/resize 等）。
    Shell(Result<ShellResponse, String>),
    /// `shell.list` 全量。
    Shells(Vec<ShellView>),
    /// `shell.open` 单条结果（UI 追加并聚焦）。
    ShellOpened(ShellView),
    /// `shell.close` 成功（UI 移除标签）。
    ShellRemoved(String),
    /// Shell PTY 下行原始字节。
    ShellBytes { shell_id: String, bytes: Vec<u8> },
    /// Shell 流生命周期。
    ShellStatus { shell_id: String, status: ShellUiStatus },
    /// Shell 操作/订阅失败。
    ShellError(String),
    /// 按内容哈希取回的内容（图片字节或文本）。
    Blob { hash: ContentHash, bytes: Vec<u8> },
    /// 附件上传完成。
    Attachment {
        session_id: SessionId,
        result: Result<PreparedAttachment, String>,
    },
    /// 引用内容审阅（diff 面板按需读全文）。
    ReviewContent {
        session_id: SessionId,
        turn: TurnId,
        request: usize,
        texts: Vec<Result<String, String>>,
    },
    /// 取消看门狗到点。
    Watchdog(String),
}

/// GPUI → 桥方向的请求。
pub enum BridgeRequest {
    Reconnect,
    /// 附着会话事件流（`since_seq` = 已收到的最后序号；`0` = 由服务端回放尾决定）。
    Select { session_id: SessionId, since_seq: u64 },
    RefreshRoster,
    /// 重灌快照 + todo + team。
    Refill(SessionId),
    /// 向上翻页。
    LoadTranscript {
        session_id: SessionId,
        before_ord: u64,
        limit: u32,
        timeline_epoch: u64,
    },
    /// 会话域直通（创建/重命名/归档/设置/目标/todo/team/技能目录……）。
    Session(SessionRequest),
    /// 命令域（`command_id` 是幂等键，重放返回原回执）。
    Command {
        command_id: String,
        request: CommandRequest,
    },
    Config(ConfigRequest),
    Account(AccountRequest),
    Workspace(WorkspaceRequest),
    ToolCatalog(ToolCatalogRequest),
    /// Shell 域直通（`Open`/`Close`/`List` 由桥附带流订阅/退订副作用）。
    Shell(ShellRequest),
    /// 重建某 tab 的 PTY 订阅（`from_offset = 0` 表示清屏重建）。
    ShellAttach { shell_id: String, from_offset: u64 },
    /// 按内容哈希取回内容（图片字节或文本）。
    FetchBlob(ContentHash),
    /// 附件落盘读取 → `upload_blob`。
    AttachFile { session_id: SessionId, path: PathBuf },
    /// 逐个内容引用取全文。
    ReviewContent {
        session_id: SessionId,
        turn: TurnId,
        request: usize,
        references: Vec<ContentRef>,
    },
    Watchdog { label: String, after_ms: u64 },
}

fn trace(msg: &str) {
    if std::env::var_os("QAQH_GPUI_TRACE").is_some() {
        eprintln!("[qaqh-gpui] {msg}");
    }
}

fn send_line(to_ui: &UnboundedSender<BridgeMsg>, text: impl Into<String>) {
    let _ = to_ui.unbounded_send(BridgeMsg::Line(text.into()));
}

fn send_shell_error(to_ui: &UnboundedSender<BridgeMsg>, text: impl Into<String>) {
    let text = text.into();
    send_line(to_ui, text.clone());
    let _ = to_ui.unbounded_send(BridgeMsg::ShellError(text));
}

/// 本机 data root（`daemon.endpoint` 所在目录）。
fn data_root() -> PathBuf {
    qaqh_client::discovery::data_dir()
}

/// 发布渠道：与 `qaqh-daemon` 的 `daemon_channel()` 同规则。
fn channel() -> String {
    std::env::var("QAQH_CHANNEL").unwrap_or_else(|_| {
        if cfg!(debug_assertions) {
            "dev".into()
        } else {
            "stable".into()
        }
    })
}

fn abort_task(slot: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(handle) = slot.take() {
        handle.abort();
    }
}

fn abort_all(map: &mut HashMap<String, tokio::task::JoinHandle<()>>) {
    for (_, handle) in map.drain() {
        handle.abort();
    }
}

/// 桥线程主循环：复用 qaqh-client 的全局 tokio 运行时。
pub fn run(
    to_ui: UnboundedSender<BridgeMsg>,
    requests: UnboundedReceiver<BridgeRequest>,
    client_instance_id: String,
) {
    qaqh_client::runtime_handle().block_on(serve(to_ui, requests, client_instance_id));
}

async fn connect(launch_if_missing: bool) -> Result<(u64, V3Client<YamuxTransport>), String> {
    let root = data_root();
    let channel = channel();
    let generation = next_generation();
    let instance = format!("qaqh-gpui-{}-{generation}", std::process::id());
    let client = V3Client::connect_local(&root, None, &channel, &instance, launch_if_missing)
        .await
        .map_err(|error| format!("V3 连接失败：{error}"))?;
    Ok((generation, client))
}

async fn call(
    client: &V3Client<YamuxTransport>,
    request: ControlRequest,
) -> Result<ControlResponse, String> {
    client.call(request).await.map_err(|error| error.to_string())
}

async fn establish(
    to_ui: &UnboundedSender<BridgeMsg>,
    blob_task: &mut Option<tokio::task::JoinHandle<()>>,
) -> Result<(u64, V3Client<YamuxTransport>), String> {
    let (generation, mut client) = connect(true).await?;
    let (server_instance_id, build_id, protocol_rev) = match client.server_hello() {
        qaqh_ringing_v3::ServerHello::Accepted {
            server_instance_id,
            build_id,
            protocol_rev,
        } => (server_instance_id.clone(), build_id.clone(), *protocol_rev),
        qaqh_ringing_v3::ServerHello::Rejected(reject) => {
            return Err(format!("握手被拒：{reject:?}"));
        }
    };
    crate::diagnostics::set_build_id(build_id.clone());
    crate::diagnostics::set_epoch(server_instance_id.clone());
    crate::diagnostics::record("connected", None, None, None);
    let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Ready {
        server_instance_id: server_instance_id.clone(),
        build_id: build_id.clone(),
    }));
    send_line(
        to_ui,
        format!("V3 已连接：server={server_instance_id} build={build_id} protocol_rev={protocol_rev}"),
    );
    abort_task(blob_task);
    if let Some(inbox) = client.take_blob_inbox() {
        *blob_task = Some(spawn_blob_inbox(to_ui, generation, inbox));
    }
    Ok((generation, client))
}

/// 桥主循环：连接 → 名册 → 请求/事件流。
async fn serve(
    to_ui: UnboundedSender<BridgeMsg>,
    mut requests: UnboundedReceiver<BridgeRequest>,
    _client_instance_id: String,
) {
    let mut client: Option<V3Client<YamuxTransport>> = None;
    let mut conn_gen = 0u64;
    let mut selected: Option<(SessionId, u64)> = None;
    let mut event_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut shell_tasks: HashMap<String, tokio::task::JoinHandle<()>> = HashMap::new();
    let mut blob_task: Option<tokio::task::JoinHandle<()>> = None;
    let mut failures = 0u32;
    let mut roster = tokio::time::interval(ROSTER_PERIOD);
    loop {
        tokio::select! {
            message = requests.next() => {
                let Some(request) = message else { break };
                match request {
                    BridgeRequest::Reconnect => {
                        abort_task(&mut event_task);
                        abort_all(&mut shell_tasks);
                        abort_task(&mut blob_task);
                        client = None;
                        conn_gen = 0;
                        failures = 0;
                        let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Connecting));
                        roster.reset_immediately();
                    }
                    BridgeRequest::Select { session_id, since_seq } => {
                        selected = Some((session_id.clone(), since_seq));
                        abort_task(&mut event_task);
                        let Some(active) = &client else { continue };
                        match attach_events(active, &to_ui, conn_gen, &session_id, since_seq).await {
                            Ok(task) => event_task = Some(task),
                            Err(error) => send_line(&to_ui, format!("附着会话事件流失败：{error}")),
                        }
                        requests::refill(active, &to_ui, &session_id).await;
                    }
                    other => {
                        let Some(active) = client.as_mut() else {
                            send_line(&to_ui, "连接尚未恢复，请稍后重试");
                            continue;
                        };
                        handle_request(active, &to_ui, conn_gen, &mut shell_tasks, other).await;
                    }
                }
            }
            _ = roster.tick() => {
                match client.as_ref() {
                    Some(active) => {
                        if requests::poll_roster(active, &to_ui).await { failures = 0; } else { failures += 1; }
                        if failures >= 3 {
                            client = None;
                            conn_gen = 0;
                            abort_task(&mut event_task);
                            abort_all(&mut shell_tasks);
                            abort_task(&mut blob_task);
                            let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Failed {
                                detail: "daemon 查询连续失败，正在重新发现服务".into(),
                            }));
                        }
                    }
                    None => {
                        let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Connecting));
                        match establish(&to_ui, &mut blob_task).await {
                            Ok((generation, active)) => {
                                conn_gen = generation;
                                failures = 0;
                                requests::poll_roster(&active, &to_ui).await;
                                if let Some((seed, since_seq)) = selected.clone() {
                                    match attach_events(&active, &to_ui, conn_gen, &seed, since_seq).await {
                                        Ok(task) => event_task = Some(task),
                                        Err(error) => send_line(&to_ui, format!("重新附着失败：{error}")),
                                    }
                                    requests::refill(&active, &to_ui, &seed).await;
                                }
                                client = Some(active);
                            }
                            Err(error) => {
                                crate::diagnostics::record("connect_failed", None, None, None);
                                let _ = to_ui.unbounded_send(BridgeMsg::Connection(ConnectionState::Failed { detail: error }));
                            }
                        }
                    }
                }
            }
        }
    }
    abort_task(&mut event_task);
    abort_all(&mut shell_tasks);
    abort_task(&mut blob_task);
}

/// `--probe`：无 GUI 连通性验证。
pub fn probe() {
    qaqh_client::runtime_handle().block_on(async {
        let (_, client) = match connect(false).await {
            Ok(connected) => connected,
            Err(error) => {
                println!("qaqh-gpui: {error}");
                return;
            }
        };
        let (build_id, protocol_rev) = match client.server_hello() {
            qaqh_ringing_v3::ServerHello::Accepted {
                build_id,
                protocol_rev,
                ..
            } => (build_id.as_str(), *protocol_rev),
            qaqh_ringing_v3::ServerHello::Rejected(reject) => {
                println!("qaqh-gpui: 握手被拒（{reject:?}）");
                return;
            }
        };
        println!("qaqh-gpui: protocol_rev={protocol_rev} build_id={build_id}");
        println!("daemon 已连接（build_id={build_id}）");
        let listed = call(
            &client,
            ControlRequest::Session(SessionRequest::List { include_archived: true }),
        )
        .await;
        match listed {
            Ok(ControlResponse::Session(SessionResponse::Sessions(list))) => {
                println!("session.list：{} 个会话", list.len());
                for summary in list.iter().take(8) {
                    println!(
                        "  · {} 「{}」",
                        summary.session_id.as_str(),
                        summary.title.as_deref().unwrap_or("(无标题)"),
                    );
                }
            }
            Ok(other) => println!("session.list 回执形状异常：{other:?}"),
            Err(error) => println!("session.list 失败：{error}"),
        }
    });
}
