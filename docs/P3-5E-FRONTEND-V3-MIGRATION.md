# P3-5e 前端迁移执行计划：`qaqh-gpui` 单 PR 原子切 RingingV3

> **状态（2026-11-27）**：前置已就绪——`../qaqh-backend` `main` 已合入 `P3-5a`～`P3-5d`（`3e4a8b35`，含 `local_ipc`、`v3_bridge`、`V3Client::connect_local`）；本仓在途改动已落盘（`e875e4a`）。
> **阻塞项**：V3 契约不够前端渲染，工单在 `../qaqh-backend/docs/v4.3-4.8/P3_5E_V3_CONTRACT_GAPS.md`。**本文件第 3 节的 A/B/C 项必须在后端补齐后才能开工**（否则 `timeline.rs` 要写两遍）。
> **可立即开工**：第 2 节（依赖与客户端接线）+ 第 3 节的 D/E/F/G/H/I（不依赖 A/B/C 的富字段）。
>
> **已完成（分支 `feat/v4.7-p3-5e-ringing-v3`，commit `6147c8c`）**：`qaqh-gpui-wire` 整层切 V3——`bridge.rs` 1316 行 → 433 行（+ `bridge/{requests,streams,attach}.rs`，单文件全部 <650），`cargo check/test -p qaqh-gpui-wire` 全绿（2 tests）。**下游 `qaqh-gpui-session` / `qaqh-gpui-shell` / `qaqh-gpui-render` 尚未迁移，全仓 `cargo check` 目前仍是红的**——这是本分支的预期中间态。

---

## 1. 目标架构（先把口径定死，避免边写边改）

1. **store 字段直持 V3 类型，零前端 DTO**（沿用本仓既有设计口径 `store.rs:1`：字段直持契约类型）。因此不引入"前端视图模型"层。
2. **时间线是前端合并态，不是协议快照**：V3 只给尾部窗口（`SessionSnapshot.tail_blocks/tail_turns`）+ 增量事件 + `TranscriptPage` 向上翻页。前端必须自己维护
   `merged_blocks: BTreeMap<u64 /*ord*/, BlockView>` + `streaming: HashMap<BlockId, (bool /*answer?*/, String)>` + `tool_calls: HashMap<ToolCallId, ToolCallView>`；
   失配（`StaleCursor` / `ResyncRequired`）**一律重取快照**，绝不拼接（`session.rs:70-71` 已写死该纪律）。
3. **大内容一律 blob**：`ContentRef::Inline` 直接渲染；`ContentRef::Blob{hash,len}` 经 `V3Client::fetch_blob` 拉取（前端做 LRU，上限 32 项，复用 `qaqh-gpui-render/src/media.rs` 的缓存思路）；`ContentRef::Windowed` 只渲染 `preview` 并标注"另有全文"。
4. **四条流各一条任务**：控制面（`ControlRequest` 一问一答）/ 会话事件（每选中会话一条 `SessionEventsStream`）/ Shell（每 tab 一条 `ShellOutputStream`）/ Blob（一条 `BlobInbox` 消费任务）。
5. **命令一律带 `command_id` 幂等键**（`ControlCall` 要求不可省略），UI 的 `Outbox` 状态机改成吃 `ControlResponse::Command`。

---

## 2. 依赖与接线（第一步，可立即做）

### 2.1 `Cargo.toml`（workspace）

```toml
# 已移除（V2 契约与 wire 类型两个 crate 已清零，见 §5 硬门禁 1）
# 新增
qaqh-ringing-v3  = { path = "../qaqh-backend/crates/qaqh-ringing-v3" }
qaqh-core-vocab  = { path = "../qaqh-backend/crates/qaqh-core-vocab" }
# 保留：qaqh-client（传输实现）、qaqh-profile（配置文档）、qaqh-v3-transport（如需自定义传输）
```

`qaqh-gpui-wire/Cargo.toml` 同步（`qaqh-client` 的 `shell-stream` feature 改按 V3 需要调整）；不再需要 `base64`（图片走 blob）。
`qaqh-gpui-render` 的 `ImageBlock` 依赖改为 `(ContentHash, mime)`，见 §3.C。

### 2.2 连接与发现（`qaqh-gpui-wire/src/bridge.rs`）

```rust
// data root / 渠道 / 构建号
let data_root = qaqh_client::discovery::data_dir();                       // 后端 re-export
let channel = std::env::var("QAQH_CHANNEL").unwrap_or_else(|_| {
    if cfg!(debug_assertions) { "dev".into() } else { "stable".into() }    // 与 daemon server.rs:13 同规则
});
// 先读端点拿到 build_id（gpui 与 daemon 不同构建，无法自带该值）
let endpoint = qaqh_ringing_v3::read_daemon_endpoint(&data_root, &channel)?;
let client = V3Client::connect_local(&data_root, &endpoint.build_id, &channel, &client_instance_id, true).await?;
```

- 删掉 `read_discovery` / `ensure_daemon_running` / `DiscoveryExt` / `Client::connect_async` / `v2_session_state`。
- `ConnectionState::Ready` 用 `client.server_hello()` 的 `server_instance_id`；`--probe` 打印 `SystemRequest::Version` 的 `build_id` + `protocol_rev`。

### 2.3 每会话事件流

```rust
let mut stream = client.attach_session(&session_id, since_seq).await?.into_frame_stream();
while let Some(frame) = stream.next().await { /* 转发 BridgeMsg::Event */ }
```

- `BridgeMsg::Stream(seed, StreamState)` 的数据源从 `V2StreamStatus` 换成 `SessionStreamStats`（`frames/last_seq/resync_cycles/ordering_violations`）。
- `BridgeMsg::Reset` **删除**：V3 的 `SessionEvent::ResyncRequired` 就是这件事，交给 reducer 处理。

### 2.4 Blob

- 上传：`client.upload_blob(&session_id, &bytes)` → `ContentHash`（附件/图片统一走这条，`prepare_attachment` 不再 base64）。
- 下载：`BridgeRequest::FetchBlob(ContentHash)` → `client.fetch_blob(&hash)` → `BridgeMsg::BlobText(hash, String)` / `BlobBytes(hash, Vec<u8>)`。
- 服务端主动推（`BlobTransferRole::ServerPush`）：`client.take_blob_inbox()` 取走收件箱，起一个长活任务消费并同上转发。

---

## 3. 类型映射表（逐文件执行的依据）

### 2.5 桥层已落地的 API 面（下游按这张表接线）

`qaqh_gpui_wire::bridge` 现在只有这些入口：

```rust
pub fn run(to_ui: UnboundedSender<BridgeMsg>, requests: UnboundedReceiver<BridgeRequest>, client_instance_id: String);
pub fn probe();
pub fn connection_generation() -> u64;

pub enum ConnectionState { Connecting, Ready { server_instance_id: String, build_id: String }, Failed { detail: String } }
pub enum DeliveryResult { Accepted { command_id: String }, Rejected(String), Unknown(String) }
pub struct PreparedAttachment { pub hash: ContentHash, pub len: u64, pub mime: String }
pub enum ShellUiStatus { Connecting, Open, Unsubscribed { reason: String }, Terminated { reason: String } }
pub struct StreamProgress { pub frames: u64, pub last_seq: u64, pub resync_cycles: u64, pub ordering_violations: u64 }

pub enum BridgeMsg {
    FromConnection(u64, Box<BridgeMsg>),
    Connection(ConnectionState),
    Line(String),
    Sessions(Vec<SessionSummary>),          // session.list（含 archived）
    Workspaces(Vec<WorkspaceEntry>),        // workspace.list
    Snapshot(Box<SessionSnapshot>),         // 尾部窗口（TAIL_BLOCKS=500 / TAIL_TURNS=60）
    Transcript(Box<TranscriptPage>),        // 向上翻页
    Event(SessionId, SessionEventFrame),    // 唯一状态真相源
    Stream(SessionId, StreamProgress),
    Session(Result<SessionResponse, String>),
    Command { session_id: Option<SessionId>, command_id: String, result: Result<CommandResponse, String> },
    Config(Result<ConfigResponse, String>),
    Account(Result<AccountResponse, String>),
    Workspace(Result<WorkspaceResponse, String>),
    ToolCatalog(Result<ToolCatalogResponse, String>),
    Shell(Result<ShellResponse, String>),
    Shells(Vec<ShellView>),
    ShellOpened(ShellView),
    ShellRemoved(String),
    ShellBytes { shell_id: String, bytes: Vec<u8> },
    ShellStatus { shell_id: String, status: ShellUiStatus },
    ShellError(String),
    Blob { hash: ContentHash, bytes: Vec<u8> },
    Attachment { session_id: SessionId, result: Result<PreparedAttachment, String> },
    ReviewContent { session_id: SessionId, turn: TurnId, request: usize, texts: Vec<Result<String, String>> },
    Watchdog(String),
}

pub enum BridgeRequest {
    Reconnect,
    Select { session_id: SessionId, since_seq: u64 },   // since_seq = 已收到的最后序号
    RefreshRoster,
    Refill(SessionId),                                  // 快照 + todos + team
    LoadTranscript { session_id: SessionId, before_ord: u64, limit: u32, timeline_epoch: u64 },
    Session(SessionRequest),                            // 创建/重命名/归档/设置/目标/todo/team/技能目录
    Command { command_id: String, request: CommandRequest },
    Config(ConfigRequest),
    Account(AccountRequest),
    Workspace(WorkspaceRequest),
    ToolCatalog(ToolCatalogRequest),
    Shell(ShellRequest),                                // Open/Close/List 带流副作用
    ShellAttach { shell_id: String, from_offset: u64 },
    FetchBlob(ContentHash),
    AttachFile { session_id: SessionId, path: PathBuf },
    ReviewContent { session_id: SessionId, turn: TurnId, request: usize, references: Vec<ContentRef> },
    Watchdog { label: String, after_ms: u64 },
}
```

下游接线要点：

| 旧调用点 | 新写法 |
|---|---|
| `BridgeRequest::SendMessage{seed, outbox}` | `BridgeRequest::Command { command_id: outbox.command_id.clone(), request: CommandRequest::SendPrompt { session_id, input_id: InputId::new(&outbox.command_id)?, text, attachments } }` |
| `BridgeRequest::RespondPermission` | `CommandRequest::RespondInteraction { resolution: CommandInteractionResolution::Approve{scope} / Reject{text} }` |
| `BridgeRequest::RespondAsk` / `DismissAsk` | 同上，`Answer{answers}` / `Dismiss` |
| `BridgeRequest::ExecControl` | `CommandRequest::ExecInterrupt{exec_session_id}` / `ExecTerminate{..}` |
| `BridgeRequest::Cancel` | `CommandRequest::CancelTurn{session_id}` |
| `BridgeRequest::CreateSession` | `SessionRequest::Create{title,cwd,tool_mode,model_selection,custom_tools,handoff_from}`，回执 `SessionResponse::Created(summary)` |
| `BridgeRequest::PatchSessionSettings` | `SessionRequest::PatchSettings{session_id,patch,expected_revision}` |
| `BridgeRequest::RenameSession` / `DeleteSession` / `MoveSession` | `SessionRequest::{Rename,Delete}` / `WorkspaceRequest::MoveSession` |
| `BridgeRequest::QueryTodo` / `QueryTeam` / `QuerySkills` | `SessionRequest::{GetTodos,GetTeam,GetSkillsCatalog}` |
| `BridgeRequest::QueryConfig` / `SaveConfig` / `ListProviders` / `ListProviderPresets` / `ManageCatalog` | `ConfigRequest::{LoadFull,PatchFull,ListProviders,ListProviderPresets,UpsertProvider,...}` |
| `BridgeRequest::Service{method,params}` | `AccountRequest::{SiwcStatus,DsStatus,DsBalance,DsBonuses,DsSignInStart,...}` |
| `BridgeRequest::ShellList/Open/Close/Stdin/Resize` | `BridgeRequest::Shell(ShellRequest::{List,Open,Close,Write,Resize})` |
| `BridgeRequest::ShellAttach` | `BridgeRequest::ShellAttach { shell_id, from_offset }` |
| `BridgeRequest::Receipt` | **删除**：V3 没有异步回执查询，`CommandResponse::Accepted` + 事件流即权威 |
| `BridgeMsg::Reset` | **删除**：用 `SessionEvent::ResyncRequired` |

> `BridgeMsg::FromConnection(generation, _)` 必须继续在 `daemon.rs` 的收件处过滤（`connection_generation()`），这是防旧连接串话的唯一机制。

### A/B/C 组（**等后端工单落地**）

| 现用（V2） | 目标（V3 + 工单 A/B/C） | 受影响文件 |
|---|---|---|
| `state::SessionState` | `SessionSnapshot` + 前端合并态（§1.2） | `reducer.rs:22-49`、`store.rs:271-406` |
| `state::TimelineEntry::{Input,AssistantBlock,ToolCall,ToolResult,Compaction}` | `BlockView`（+ 工单 B 字段）+ `pinned_compaction` | `timeline.rs:3125-3400`、`turn_rail.rs:58-61`、`reducer.rs:408-455` |
| `state::ToolCallView{args,state,output,diff,metrics,code_delta,effects}` | `stream::ToolCallView`（+ 工单 A 字段） | `timeline.rs:1662-2000,2066-2189`、`code_panel.rs:423-1300`、`store.rs:142-211` |
| `fact::{AssistantBlockKind,BlockId}` | `core_vocab::BlockId` + `BlockKind` | `store.rs:73-85,296-297`、`timeline.rs:2745` |
| `fact::ContentValue / ContentRef` | `core_vocab::ContentRef`（Inline/Windowed/Blob） | 全局 `content_text()`：`timeline.rs:2829-2842`、`store.rs:150` |
| `commands::ImageBlock` | `ImageRefView{hash,len,mime}` + `fetch_blob` | `render/media.rs:3-51`、`store.rs:274`、`turn_rail.rs:83-91` |

### D 组（可立即做）

| 现用 | 目标 | 文件 |
|---|---|---|
| `state::ExecSessionView` | `ExecSessionSnapshot`（+ 工单 D：`state/output_preview/preview_start_offset`） | `terminal_panel.rs:226-328`、`timeline.rs:1775`、`store.rs:368-370` |
| `events::{ExecSessionStarted,StateChanged,Finished}` | `SessionEvent::ExecSessionUpdated` | `reducer.rs:365-391` |

### E 组（可立即做）

| 现用 | 目标 | 文件 |
|---|---|---|
| `state::InteractionView.request`(JSON) + `choices` | `InteractionView{title,body,questions,options}` + 工单 E `permission/choices` | `ask_dock.rs:241-245`、`timeline.rs:3402-3450` |
| `AskAnswer{question_id,answer}` | `stream::AskAnswer`（同形，直接用） | `bridge.rs:711-726`、`ask_dock.rs` |
| `InteractionId` newtype | `core_vocab::InteractionId` | 同上 |

### F 组（可立即做）

| 现用 | 目标 | 文件 |
|---|---|---|
| `qaqh_types::SessionListEntry` | `SessionSummary`（+ 工单 F：`custom_tools`/`created_at_ms`） | `store.rs:1469-1490`、`sidebar.rs:249`、`bridge.rs:417-424` |
| `qaqh_types::SessionRunStatus` | `core_vocab::ActivityState`（四态，`NotRunning` 语义废除） | `timeline.rs:803-819` |
| `state::{GoalView,GoalStatus}` | `team::{GoalView}` + `core_vocab::GoalStatus` | `goal.rs`、`store.rs:329-330` |
| `state::SubagentView` | `team::SubagentSummary` | `store.rs:328`、`handoff.rs` |
| `workspace::WorkspaceEntry` | `workspace::WorkspaceEntry`（V3 同名同形） | `store.rs:36,1476`、`sidebar.rs` |
| `todo::TodoListSnapshot` | `team::TodoListSnapshot` | `store.rs:398,1497` |
| `team::snapshot/task_types` | `team::{TeamOverview,TeamAgentStatus,TeamAgentResidency}` | `store.rs`、`pages.rs` |

### G 组（可立即做，事件 vocabulary）

| 现用事件 | 目标 | 备注 |
|---|---|---|
| `MessageDelta{block_kind,text}`（**全量替换**语义） | `AnswerDelta{block_id,text}` / `ReasoningDelta`（**增量**语义） | reducer 改成 `push_str`，tok/s 采样改按增量长度 |
| `ToolDeclared{args}` / `ToolStateChanged` / `ToolFinished{...}` | `ToolCallUpdated{call}` | 全量替换该 call 的视图 |
| `TurnEnded{outcome}` | `TurnFinished{terminal,usage}` + `TurnSummary` | `TurnOutcome` → `TurnTerminal` 四态 |
| `HandoffSealed{work_summary,turn_id,at_ms}` | `HandoffSealed{successor_session_id,summary:ContentRef}` | 摘要取 `Inline` 文本（工单已注明） |
| `HookStarted/HookFinished` | **删除** | 无消费方 |
| `TimelineTruncated{removed_turn_ids}` | `TimelineTruncated{from_turn_id,timeline_epoch}` + 工单 G `removed_turn_ids` | — |

### H 组（可立即做）

| 现用 | 目标 | 文件 |
|---|---|---|
| `RingingCommand::ConfigSave/SessionCreate/...` | `ConfigRequest::{PatchFull,UpsertProvider,...,SetDefaultModel}`、`SessionRequest::{Create,Rename,PatchSettings,...}` | `settings_ui.rs:165-173`、`views.rs:956`、`store.rs:1105-1230` |
| `query_typed::<ConfigDto>(QueryRequest::ConfigLoad)` | `ConfigRequest::LoadFull` → `ConfigResponse::FullDocument{raw}` → `serde_json::from_str::<ConfigDto>` | `bridge.rs:1082` |
| `RingingCommand::ProviderList` | `ConfigRequest::ListProviders` → `Providers{raw,revision}` → `ProfilesDocument` | `bridge.rs:1023`、`settings_ui.rs` |
| `ProviderPresetView{wire: Wire, credential: CredentialSource}` | V3 `ProviderPresetView`（工单 H：字符串改枚举） | `settings_ui.rs`、`bridge.rs:1050` |
| `qaqh_types::ModelSelection` | `core_vocab::ModelSelection` | `store.rs:1134,1751-1770`、`composer.rs` |
| `qaqh_types::SettingValue` | `core_vocab::SettingOverride<T>` | `store.rs:1150` |
| `qaqh_types::ContextUsage` | 前端自带（10 行）或 V3 补 `ContextUsageView` | `store.rs:1318` |
| `qaqh_ringing_contract::ulid::generate_ulid` | `command_id` 由前端生成（`ContentHash` 无关）：用 `ClientHello.client_instance_id` + 原子计数拼即可，勿再拉 ULID crate | `store.rs:601`、`bridge.rs` |

### I 组（可立即做）

| 现用 | 目标 | 文件 |
|---|---|---|
| `BridgeRequest::Service{method,params}` + `client.service_call` | `AccountRequest::{SiwcStatus,SiwcSignInStart,DsStatus,DsBalance,...}` + `AccountResponse::*View` | `daemon.rs:960-1090,1101-1300`、`settings_ui.rs:541-900` |
| `qaqh_client::service_types::{SiwcStatus,DsAccountStatus,...}` | `qaqh_ringing_v3::account::{SiwcStatusView,DsAccountStatusView,DsBalanceView,DsBonusView,DsAccountModelView}` | 同上 |

### Shell（可立即做）

| 现用 | 目标 |
|---|---|
| `service_v2("shell.list"/"shell.open"/"shell.close")` | `ShellRequest::{List,Open,Close}` → `ShellResponse::{List,Opened,Closed}` + `ShellView` |
| `ShellStream`（ws + 自动重拨） | `client.attach_shell(shell_id, from_offset)` + `ShellOutputStream::next_frame`；重连用 `stats().last_offset` 作 `from_offset` |
| `ShellStreamStatus` | 前端 `ShellUiStatus` 由流任务状态直接产出（`Connecting/Open/Terminated/Unsubscribed`） |
| `stdin/resize` | `ShellRequest::{Write,Resize}`（`session_id: Option<SessionId>` 见 `control.rs:287-332`） |

---

## 4. 文件工单（**全部已落地**，本节只留记录）

1. ✅ `crates/qaqh-gpui-wire`：`bridge.rs` 1316 → 435 行（+ `bridge/{requests,streams,attach}.rs`）；`ask.rs` 删除；`delivery.rs` 改吃 `CommandResponse`；`diagnostics.rs` 切协议自证字段；新增 `ids.rs`（本地 id）与 `profile_vocab.rs`（配置文档盘上字符串 ↔ V3 词表）。
2. ✅ `crates/qaqh-gpui-session/src/{store.rs,reducer.rs}`：合并态模型（`BlobCache`/`pending_send`/`pending_first_message`/`transcript_before_ord`）+ V3 事件 reducer（真增量 delta、`seq` 水位、`timeline_epoch` 失配整份重取）。
3. ✅ `crates/qaqh-gpui-session/src/timeline.rs`：`BlockView` + `ToolCallView` 渲染，审批卡按钮组吃 `InteractionView.choices`。
4. ✅ 会话侧其余模块（`code_panel/terminal_panel/ask_dock/goal/handoff/turn_rail/controls/persist/composer/markdown`）逐处替换。
5. ✅ `crates/qaqh-gpui-render/src/media.rs`：图片从 base64 改 `(ContentHash, mime)` + blob 字节解码缓存。
6. ✅ `crates/qaqh-gpui-shell/src/*`：账号域改 `AccountRequest`；配置域改 `ConfigRequest`；Shell 改 `ShellRequest`/`ShellView`；侧栏吃 `SessionSummary`。
7. ✅ `crates/qaqh-gpui/src/{app.rs,acceptance.rs,main.rs}`：probe/bootstrap 文案与请求面改 V3（`server_instance_id`/`protocol_rev`）。
8. ✅ 清理：`Cargo.toml` 删 V2 契约 crate、wire 类型 crate 与 `base64`；`docs/m1-draft` 死草稿删除。

---

## 5. 验收（一条都不许跳）

```bash
cd /home/qaqtam/项目/qaqh-gpui
# 硬门禁 1：旧契约(crate 名)与回退开关(宏名)共三个标识全仓 0 命中
#   rg '<V2 契约 crate>|<V2 类型 crate>|<V2 回退开关宏>' -g '!target' -g '!Cargo.lock' .
# 硬门禁 2：bridge 腰斩
wc -l crates/qaqh-gpui-wire/src/bridge.rs     # 必须 <= 650
# 硬门禁 3：全仓编译与测试（8 核 / 8 GiB，严禁全仓 --release）
cargo check -j2 --workspace
cargo test -j2 --workspace
# 硬门禁 4：连通性（无 GUI）
cargo run -j2 -p qaqh-gpui -- --probe
# 硬门禁 5：真窗口端到端（人工）
#   建会话 → 发消息 → 看答案流/工具卡/diff 面板 → 审批卡 → Ask 卡 →
#   终端坞开 shell 并输入 → 后台执行观察窗 → 上翻历史（TranscriptPage）→
#   切会话/切回（尾窗+增量续接）→ 图片附件预览 → 设置页改模型 → 账号页登录态
```

### 验收记录（2026-11-28，P3-5e 落地当轮）

| 门禁 | 结果 |
|---|---|
| 1 旧契约零命中 | ✅ `rg` 无输出（含 docs；`docs/m1-draft` 死草稿已删） |
| 2 bridge 行数 | ✅ 435 行（≤ 650） |
| 3 编译/测试 | ✅ `cargo check -j2 --workspace --all-targets` 0 错；`cargo test -j2 --workspace` 26 通过 0 失败；`cargo check -p qaqh-gpui --features acceptance` 绿 |
| 4 连通性 | ✅ 隔离数据根跑 V3 daemon：`protocol_rev=1 build_id=4.2.0` / `daemon 已连接` / `session.list：0 个会话` |
| 5 真窗口 | ⏳ 待人工（见下方清单） |

---

## 6. 已知坑（写在这里免得下一轮再踩）

1. **`AnswerDelta` 是增量、V2 `MessageDelta` 是全量替换**——照抄旧 reducer 会把答案拼成乱码。
2. **`SessionSnapshot` 是尾窗**，`timeline_epoch` 失配必须整份重取；`tail_blocks` 里没有的块只能靠 `TranscriptPage` 拉，不能假装它不存在。
3. **`attach_session(since_seq)` 的 `since_seq` 是"已收到的最后序号"**，不是"下一个"；`SessionSnapshot.seq` 也是"下一个将分配"的互斥上界（`session.rs:18-21`）。
4. **Shell 偏移**：`ShellOutputStream::attach().seq` 是服务端当前水位；重连清屏重建时用它，不要拿 `stats().last_offset` 当"已显示"。
5. **`StreamRejected`/`HandshakeTimeout` 必须走重连节奏**，桥线程一死 UI 就僵死（现有 `failures >= 3` 逻辑保留）。
6. **`V3Client::take_blob_inbox` 是一次性取走**（`&mut self`），要在连接建立后立刻取，否则服务端推的 blob 无处落地。
