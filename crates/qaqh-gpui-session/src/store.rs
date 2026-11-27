//! GPUI 实体状态(M3):字段直持 RingingV3 契约类型,零前端 DTO。
//! 多会话:visited store 常驻缓存、事件按 seed 路由;输入框统一走
//! gpui-component 的 InputState/TextareaState(InputState::new 需要
//! &mut Window,故实体构造一律在窗口上下文内完成)。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use futures::channel::mpsc::UnboundedSender;
use gpui_kit::App;
use gpui_kit::AppContext as _;
use gpui_kit::Context;
use gpui_kit::Entity;
use gpui_kit::FocusHandle;
use gpui_kit::Focusable;
use gpui_kit::ScrollHandle;
use gpui_kit::Subscription;
use gpui_kit::Window;
use gpui_kit::{ListAlignment, ListState, px};
use qaqh_core_vocab::{
    ActivityState, BlockId, ContentRef, ExecSessionId, GoalStatus, InputId, InteractionId,
    ModelSelection, SessionId, SettingOverride, ToolCallId, ToolMode, TurnId, UsageInfo,
    WorkspaceId,
};
use qaqh_gpui_wire::bridge::BridgeRequest;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use qaqh_profile::ProfilesDocument;
use qaqh_ringing_v3::{
    AccountRequest, AskAnswer, BlockKind, BlockView, CompactionSummary, ExecSessionSnapshot,
    GoalView, ImageRefView, InteractionView, SessionEventFrame, SessionRequest, SessionSummary,
    SessionSettingsPatch, SubagentSummary, ToolCallView, TurnSummary, WorkspaceEntry,
};

/// 单会话 SSE 流状态(由 `V2StreamStatus` 映射,避免 store 依赖 SDK 内部枚举形状)。
pub enum StreamState {
    Connecting,
    Open,
    Reconnecting { retry_ms: u64 },
    Closed { reason: String },
}

/// ask 卡草稿:题号列表 + 步进页码 + 折叠态 + 卡片焦点 + 每题已选项/自定义输入框。
pub struct AskDraft {
    /// 与请求对齐的题号列表;仅在题号列表变化时重建草稿。
    pub question_ids: Vec<String>,
    /// 每题点选的选项文本(与 customs 互斥生效)。
    pub answers: Vec<Option<String>>,
    /// 每题自定义输入框(非空时优先于 answers)。
    pub customs: Vec<Entity<InputState>>,
    /// 当前激活题索引(0-based)。
    pub active_index: usize,
    /// 是否折叠为单行提醒条。
    pub collapsed: bool,
    /// Ask 卡片键盘导航焦点句柄。
    pub focus_handle: FocusHandle,
    /// 自定义输入框订阅,随草稿释放自动清理。
    pub _subscriptions: Vec<Subscription>,
}

/// 参数生成中的流式摘要(ToolArgumentsDelta 的投影;声明后由 ToolDeclared 收口)。
/// 行数是内核从参数文本数的**估算**,终态以 ToolFinished.code_delta 为准。
pub struct ArgStream {
    pub call_id: ToolCallId,
    pub tool_name: String,
    pub text: String,
    /// (估算新增行, 估算删除行);None = 该工具没有行数概念。
    pub lines: Option<(u32, u32)>,
    /// 内核认出的目标文件(仅 apply_patch 会填;缺席 = 没认出来)。
    pub target_files: Vec<String>,
    /// 参数文本已吐完且形状合法(不代表已声明/已执行)。
    pub args_complete: bool,
}

/// 会话内向外壳派发的导航请求(如封箱飞箱跳后继、开箱「去源头」跳前驱)。
#[derive(Clone, Debug)]
pub enum SessionNavEvent {
    OpenSession {
        session_id: SessionId,
        fly_from_session: Option<SessionId>,
    },
    SidePanelOpenChanged {
        open: bool,
    },
    OpenAccess,
    /// 交接分叉:壳层建会话并在 `Created` 回执后投递首条 `SendPrompt`(交接文档)。
    HandoffFork {
        title: Option<String>,
        doc: String,
    },
    /// 会话已被 daemon 删除(壳层据此关闭/淘汰对应 store)。
    SessionRemoved { session_id: SessionId },
}

impl gpui_kit::EventEmitter<SessionNavEvent> for SessionStore {}

/// ADR-0016 封箱投影:标记旧线已封箱所需的后继入口与工作简述。
/// HandoffSealed 双发(封箱线/后继线),仅封箱线本线落库;整字段替换幂等。
/// bootstrap 快照不携带本字段;封箱是不可逆历史事实,resync 后保留。
#[derive(Clone, Debug)]
pub struct HandoffSeal {
    pub sealed_session_id: String,
    pub successor_session_id: String,
    pub work_summary: String,
    pub at_ms: u64,
}

/// ADR-016 交接卡事实:fork 文档/简述取 args(模型亲手写的结构化);T0/计划
/// 开箱/自动接续取 output 机器面(5fa1762d),偏好 `args.auto_continue` 优先。
/// 非布尔/缺省一律 true——字段缺省 ≠ 模型声明「不拽我」。机器面读不出时
/// `sealed=false` 仍给卡片(electron inferred 路径同口径:入口照给,出处标出)。
#[derive(Clone, Debug)]
pub struct HandoffFacts {
    pub call_id: ToolCallId,
    pub summary: Option<String>,
    pub doc: String,
    /// 机器面 `handed_off === true`(权威封箱标记)。
    pub sealed: bool,
    pub sealed_at_ms: Option<u64>,
    pub planned_open_at_ms: Option<u64>,
    pub auto_continue: bool,
    pub context_used_tokens: Option<u64>,
    pub context_window_tokens: Option<u64>,
    pub modified_files: Vec<String>,
}

/// 毫秒钟(倒计时/开箱判定);daemon 与前端同机,unix ms 直接比对。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// handoff 工具调用 → 交接事实。主判据 = 机器面 `handed_off === true`;
/// 文档正文优先取 args(output 里的同名字段只作冗余兜底),没有正文不配叫交接。
pub fn handoff_facts(tool: &ToolCallView, text_of: impl Fn(&ContentRef) -> String) -> Option<HandoffFacts> {
    if tool.tool_name.as_str() != "handoff" {
        return None;
    }
    if tool.status != qaqh_ringing_v3::ToolCallStatus::Succeeded {
        return None;
    }
    let args: serde_json::Value = serde_json::from_str(&text_of(tool.args.as_ref()?)).ok()?;
    let doc = args.get("handoff_doc")?.as_str()?.to_string();
    if doc.trim().is_empty() {
        return None;
    }
    let summary = args
        .get("work_summary")
        .and_then(|value| value.as_str())
        .map(str::to_owned)
        .filter(|text| !text.trim().is_empty());
    let payload = tool
        .output
        .as_ref()
        .and_then(|output| extract_machine_json(&text_of(output)));
    let sealed = payload
        .as_ref()
        .is_some_and(|payload| payload.get("handed_off") == Some(&serde_json::Value::Bool(true)));
    let arg_auto = args.get("auto_continue").and_then(|value| value.as_bool());
    let output_auto = payload
        .as_ref()
        .and_then(|payload| payload.get("auto_continue"))
        .and_then(|value| value.as_bool());
    let string_list = |value: Option<&serde_json::Value>| -> Vec<String> {
        value
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut modified_files = string_list(payload.as_ref().and_then(|p| p.get("modified_files")));
    if modified_files.is_empty() {
        modified_files = string_list(args.get("modified_files"));
    }
    Some(HandoffFacts {
        call_id: tool.tool_call_id.clone(),
        summary,
        doc,
        sealed,
        sealed_at_ms: payload
            .as_ref()
            .and_then(|payload| payload.get("sealed_at_ms"))
            .and_then(|value| value.as_u64()),
        planned_open_at_ms: payload
            .as_ref()
            .and_then(|payload| payload.get("planned_open_at_ms"))
            .and_then(|value| value.as_u64()),
        auto_continue: arg_auto.or(output_auto).unwrap_or(true),
        context_used_tokens: payload
            .as_ref()
            .and_then(|payload| payload.get("context_used_tokens"))
            .and_then(|value| value.as_u64()),
        context_window_tokens: payload
            .as_ref()
            .and_then(|payload| payload.get("context_window_tokens"))
            .and_then(|value| value.as_u64()),
        modified_files,
    })
}

/// 取机器面 JSON(electron handoffPayloadFromOutput 同口径):输出是「前半句
/// 人话 + ```json 机器面」;整段本身是 JSON 也吃;扫平衡对象段,带
/// `handed_off` 的段优先,否则留第一段(文档正文里出现的 JSON 不会带这个键)。
fn extract_machine_json(output: &str) -> Option<serde_json::Value> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
        return Some(value);
    }
    let bytes = output.as_bytes();
    let mut first = None;
    for start in 0..bytes.len() {
        if bytes[start] != b'{' {
            continue;
        }
        let (mut depth, mut in_string, mut escaped) = (0i32, false, false);
        for (offset, &byte) in bytes[start..].iter().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }
            match byte {
                b'\\' if in_string => escaped = true,
                b'"' => in_string = !in_string,
                b'{' if !in_string => depth += 1,
                b'}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(slice) = output.get(start..=start + offset)
                            && let Ok(value) = serde_json::from_str::<serde_json::Value>(slice)
                        {
                            let with_seal = value.get("handed_off").is_some();
                            if first.is_none() {
                                first = Some(value.clone());
                            }
                            if with_seal {
                                return Some(value);
                            }
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    first
}

/// Provider 请求重试态(ADR-0018 展示面):仅展示、不落库、不进事实;
/// 任何权威帧到达即由 reducer 清除(帧词表:重试中永远不与已展示文本并存)。
#[derive(Clone, Debug)]
pub struct ProviderRetry {
    pub attempt: u32,
    pub max_attempts: u32,
    pub delay_secs: u64,
    pub error: String,
}

/// blob 缓存条目上限(图片与全文共用);超限按插入序淘汰。
const BLOB_CACHE_LIMIT: usize = 64;

/// 向上翻页单页块数。
pub const TAIL_PAGE: u32 = 200;

/// 待确认的用户输入(`SendPrompt` 的载荷)。
///
/// V3 的 `Outbox` 只留幂等键与投递态(投递态是 UI 自己的状态机),输入内容由这里
/// 保存:重试必须复用同一个 `input_id` 与同一组内容,被拒时要把原文回填输入框。
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PendingSend {
    pub input_id: InputId,
    pub text: String,
    pub attachments: Vec<ContentRef>,
    pub images: Vec<ImageRefView>,
}

/// 内容寻址 blob 缓存:图片字节与文本全文同一来源,按 sha256 去重。
///
/// V3 的大内容一律走 `BlobTransfer`(禁止 base64 内联),渲染层只能拿到
/// `ContentRef::Blob { hash, len }`;这里保存已到货的字节,未到货的哈希登记为
/// `pending`,由 `SessionStore::prefetch_blobs` 发一次 `FetchBlob`。
#[derive(Default)]
pub struct BlobCache {
    entries: HashMap<qaqh_core_vocab::ContentHash, Arc<[u8]>>,
    order: std::collections::VecDeque<qaqh_core_vocab::ContentHash>,
    pending: HashSet<qaqh_core_vocab::ContentHash>,
}

impl BlobCache {
    pub fn get(&self, hash: &qaqh_core_vocab::ContentHash) -> Option<Arc<[u8]>> {
        self.entries.get(hash).cloned()
    }

    pub fn put(&mut self, hash: qaqh_core_vocab::ContentHash, bytes: Vec<u8>) {
        self.pending.remove(&hash);
        if self.entries.insert(hash, Arc::from(bytes.into_boxed_slice())).is_some() {
            return;
        }
        self.order.push_back(hash);
        while self.order.len() > BLOB_CACHE_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    /// 首次见到该哈希时登记并返回 `true`(调用方据此发一次 `FetchBlob`)。
    pub fn want(&mut self, hash: &qaqh_core_vocab::ContentHash) -> bool {
        !self.entries.contains_key(hash) && self.pending.insert(*hash)
    }
}

pub struct SessionStore {
    pub snapshot_pending: bool,
    pub buffered_frames: Vec<SessionEventFrame>,
    pub outbox: Option<qaqh_gpui_wire::delivery::Outbox>,
    /// 与 `outbox` 成对的输入载荷(重试/回填用)。
    pub pending_send: Option<PendingSend>,
    /// 新会话的首条消息(交接文档/启动页输入):快照落定后自动投递。
    pub pending_first_message: Option<String>,
    pub images: Vec<ImageRefView>,
    pub attachments: Vec<ContentRef>,
    pub attachment_pending: bool,
    pub operation_error: Option<String>,
    pub output_budget: usize,
    pub search: Entity<InputState>,
    pub search_open: bool,
    /// 左侧轮次导轨感应状态(对话 >= 2 轮且鼠标靠近时间线左缘时显现)。
    pub turn_rail_near_left: bool,
    pub turn_rail_hovered: bool,
    pub turn_rail_hovered_idx: Option<usize>,
    pub turn_rail_last_idx: Option<usize>,
    pub code_panel: crate::code_panel::CodePanel,
    pub session_id: SessionId,
    pub title: Option<String>,
    /// 活动状态。(V3 只有四态,不存在"未运行/未加载";快照到达前按 `Idle`。)
    pub activity: ActivityState,
    pub current_turn_id: Option<TurnId>,
    /// 已收到的最后事件序号(`attach_session(since_seq)` 的入参)。
    pub seq: u64,
    /// 时间线代际(翻页校验;`TimelineTruncated` 递增)。
    pub timeline_epoch: u64,
    pub turns: Vec<TurnSummary>,
    /// 前端合并态:快照尾部窗口 + `TimelineAppended` + 翻页,按 `ord` 升序。
    pub entries: Vec<BlockView>,
    /// 未 seal 的助手流式块:block_id → (kind, 累计追加文本)。
    pub streaming: Vec<(BlockId, BlockKind, String)>,
    /// 工具调用业务状态(单一权威,与 timeline 的 ToolCall 条目按 call_id 关联)。
    pub tools: Vec<ToolCallView>,
    pub expanded_tools: HashSet<ToolCallId>,
    /// 展开的思考链路块(默认折叠,同 expanded_tools 口径)。
    pub expanded_thinking: HashSet<BlockId>,
    /// 展开的连续工具轨迹组(以组内首条 entry 索引为 key)。
    pub expanded_groups: HashSet<usize>,
    /// 折叠模式下用户手动展开的已收口过程视窗(以稳定 trace_key 为 key)。
    pub expanded_traces: HashSet<String>,
    /// 折叠模式下用户手动收起的活跃过程视窗(以稳定 trace_key 为 key)。
    pub collapsed_traces: HashSet<String>,
    /// 过程视窗的内部滚动句柄(按 trace_key 保持滚动位置与自动粘底)。
    pub trace_scroll_handles: HashMap<String, ScrollHandle>,
    /// 过程视窗实时计时锚点(仅活跃期记录,收口时冻结进 `trace_elapsed_ms`)。
    pub trace_started_at: HashMap<String, Instant>,
    /// 已收口过程视窗的实测耗时(ms)。
    pub trace_elapsed_ms: HashMap<String, u64>,
    /// 回合实时计时锚点(收到 `TurnStarted` 或首条活跃事件时记录,仅前端补位)。
    pub turn_started_at: HashMap<TurnId, Instant>,
    /// 已结束回合的实测耗时(ms)。
    pub turn_elapsed_ms: HashMap<TurnId, u64>,
    /// 1s 计时刷新 ticker 是否已武装(会话非 Running 即停)。
    pub trace_ticking: bool,
    /// 用户手切过的消息区原位 Todo 卡片(相对默认 `total <= 12` 展开规则取反)。
    pub toggled_todo_cards: HashSet<ToolCallId>,
    /// 未 declare 的参数生成流:call_id → 流式摘要。
    pub arg_stream: Vec<ArgStream>,
    /// 运行中工具的输出尾窗:call_id → 累计文本。
    pub output_tail: Vec<(ToolCallId, String)>,
    pub interactions: Vec<InteractionView>,
    pub subagents: Vec<SubagentSummary>,
    /// 会话目标(ADR-0015):bootstrap + goal_updated 事件维护。
    pub goal: Option<GoalView>,
    /// 固定压缩摘要(独立于回合窗口)。
    pub pinned_compaction: Option<CompactionSummary>,
    /// 当前驱动权客户端实例 id 与定序序号(多端联动)。
    pub driver_client_instance_id: Option<String>,
    pub driver_ordinal: u64,
    /// 内容寻址 blob 缓存(图片字节与全文同源)。
    pub blobs: BlobCache,
    /// 向上翻页游标(`None` = 还没翻过,首屏窗口就是最新)。
    pub transcript_before_ord: Option<u64>,
    pub transcript_loading: bool,
    /// 封箱事实(ADR-0016):HandoffSealed 事件维护;UI 投影(封箱标记/后继入口)读此。
    pub handoff_seal: Option<HandoffSeal>,
    /// 开箱机器面(ADR-016/5fa1762d):handoff 工具终态成功时解析;倒计时投影。
    pub handoff_open: Option<HandoffFacts>,
    /// 自动接续待跳(封箱线正开着且模型未声明「不拽我」):HandoffSealed 落值,
    /// daemon 消费后清零。
    pub handoff_auto_jump: Option<String>,
    /// 倒计时 ticker 已武装(防重复 spawn)。
    pub handoff_ticking: bool,
    /// 跨组件飞箱坐标锚点表(壳层与 SessionStore 共享)。
    pub handoff_anchors: crate::handoff::HandoffAnchors,
    /// 来自目录血缘的前驱会话 id(ADR-016 切片 2)。
    pub handoff_from_session_id: Option<SessionId>,
    /// 本会话创建时刻(unix ms;开箱卡兜底时间源)。
    pub session_created_at_ms: u64,
    /// 通过 HandoffSealed 双发到达的后继线权威开箱来源。
    pub handoff_incoming: Option<crate::handoff::HandoffOpenSource>,
    /// 「不拽我」标记:用户点取消自动或模型声明 `auto_continue:false`。
    pub handoff_stay: bool,
    /// 用户点「立即继续」:后继一落盘立即飞箱跳转。
    pub handoff_eager: bool,
    /// A 侧展开查看交接文档的 call_id 集合。
    pub handoff_doc_open: HashSet<ToolCallId>,
    /// B 侧开箱卡正文是否展开全文。
    pub handoff_open_expanded: bool,
    /// B 侧开箱卡是否切换为纯文本原文视图。
    pub handoff_open_raw: bool,
    /// A 侧正在播放的大盒封箱仪式(call_id -> 启动时刻)。
    pub handoff_seal_rituals: std::collections::HashMap<ToolCallId, std::time::Instant>,
    /// 已播完或已历史定格的封箱仪式(切回不重播)。
    pub handoff_ritual_done: HashSet<ToolCallId>,
    /// B 侧正在播放的大盒开箱仪式启动时刻。
    pub handoff_open_ritual: Option<std::time::Instant>,
    /// B 侧开箱仪式是否已结束/定格(切回不重播)。
    pub handoff_open_ritual_done: bool,
    /// Provider 请求重试中(ADR-0018):横幅投影;权威帧到达即清。
    pub provider_retry: Option<ProviderRetry>,
    pub exec_sessions: Vec<ExecSessionSnapshot>,
    /// 执行会话输出:exec_session_id → (已收 offset 上界, 累计文本)。
    pub exec_output_tail: Vec<(ExecSessionId, u64, String)>,
    pub stream: StreamState,
    /// Markdown 扩展插件句柄(每会话固定一次,避免每帧触发 TextView 重解析)。
    pub md_extensions: Arc<gpui_kit::base::text::MarkdownExtensions>,
    /// 后台执行终端面板(pty 捕获流 → 只读 ANSI 屏)。
    pub terminal_panel: crate::terminal_panel::TerminalPanel,
    /// 流式吐字速率采样:3s 滑窗,元素 = (到达时刻, 本次净增字符数)。
    /// 速率为估算(字符/4≈token);封口块由 usage/generated_ms 给权威值。
    pub tok_samples: std::collections::VecDeque<(std::time::Instant, i64)>,
    pub timeline_list: ListState,
    pub timeline_follow_setting: Option<bool>,
    pub settings: Option<SessionSummary>,
    pub settings_pending: bool,
    /// 会话当前工具模式(composer 模式切换的权威值)。
    pub tool_mode: ToolMode,
    pub catalog: Option<Arc<ProfilesDocument>>,
    pub siwc_ready: bool,
    pub dsaccount_signed_in: bool,
    /// composer 状态(M2):组件库 Textarea(auto_grow,Enter 提交,Shift+Enter 换行)。
    pub composer: Entity<TextareaState>,
    /// 重命名草稿(Some = 重命名模式)。
    /// ask 卡草稿,按 interaction_id。
    pub ask_drafts: Vec<(InteractionId, AskDraft)>,
    /// ask 卡未答提示(上次提交被拒时显示,成功提交/跳过/解决后清除)。
    pub ask_hint: Option<String>,
    /// goal Dialog 草稿(objective, 预算);Some = 对话框打开中。
    pub goal_draft: Option<(Entity<InputState>, Entity<InputState>)>,
    /// 待二段确认撤回的 turn_id(conversation.truncate)。
    pub confirm_truncate_turn: Option<String>,
    /// 当前会话任务清单(内联悬挂在消息区右上角圆角卡片)。
    pub todos: Vec<TodoRow>,
    /// 消息区悬挂任务卡片开关(顶部「任务」按钮与卡片 × 均可自由切换)。
    pub todo_card_open: bool,
    /// Composer 模型选择器「管理」弹出卡片开关与行内输入框。
    pub model_params_open: bool,
    pub model_params_max_output: Option<Entity<InputState>>,
    pub model_params_custom_tools: Option<Entity<InputState>>,
    pub requests: UnboundedSender<BridgeRequest>,
}

impl SessionStore {
    #[cfg(feature = "acceptance")]
    pub fn simulate_closed_bridge(&mut self, cx: &mut Context<Self>) {
        let original = self.requests.clone();
        let (tx, rx) = futures::channel::mpsc::unbounded();
        drop(rx);
        self.requests = tx;
        self.composer_send(cx);
        self.requests = original;
    }
    pub fn requests(&self) -> &UnboundedSender<BridgeRequest> { &self.requests }
    pub fn new(
        session_id: SessionId,
        requests: UnboundedSender<BridgeRequest>,
        draft: crate::persist::Draft,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 8)
                .submit_on_enter(true)
                .placeholder("输入消息…")
        });
        // Enter(无 Shift)提交;Shift+Enter 由引擎插入换行(submit_on_enter 语义)。
        cx.subscribe(&composer, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                this.composer_send(cx);
            }
            cx.notify();
        })
        .detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索消息…"));
        // 搜索结果随输入实时过滤,Change 驱动父视图重渲染。
        // 事件类型需显式标注:InputState 同时发 InputEvent/InputEditsEvent/NumberInputEvent。
        cx.subscribe(&search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        if !draft.text.is_empty() {
            composer.update(cx, |state, cx| state.set_value(draft.text.as_str(), window, cx));
        }
        // 滚动条跟随滚动位置重绘:滚动帧经 defer 通知实体(gpui list 的内部
        // 借用释放后再 update,MessageScroller 同款防借用模式)。
        let timeline_list = ListState::new(1, ListAlignment::Top, px(200.));
        let weak_store = cx.weak_entity();
        timeline_list.set_scroll_handler(move |_, _, cx| {
            let weak_store = weak_store.clone();
            cx.defer(move |cx| {
                let _ = weak_store.update(cx, |_, cx| cx.notify());
            });
        });
        Self {
            snapshot_pending: true,
            buffered_frames: Vec::new(),
            code_panel: crate::code_panel::CodePanel::with_root(draft.local_root),
            outbox: draft.outbox,
            pending_send: draft.pending_send,
            pending_first_message: None,
            images: draft.images,
            attachments: draft.attachments,
            attachment_pending: false,
            operation_error: None,
            output_budget: qaqh_gpui_kit::settings::get(cx).behavior.output_tail_max,
            search,
            search_open: false,
            turn_rail_near_left: false,
            turn_rail_hovered: false,
            turn_rail_hovered_idx: None,
            turn_rail_last_idx: None,
            session_id,
            title: None,
            activity: ActivityState::Idle,
            current_turn_id: None,
            seq: 0,
            timeline_epoch: 0,
            turns: Vec::new(),
            entries: Vec::new(),
            streaming: Vec::new(),
            tools: Vec::new(),
            expanded_tools: HashSet::new(),
            expanded_thinking: HashSet::new(),
            expanded_groups: HashSet::new(),
            expanded_traces: HashSet::new(),
            collapsed_traces: HashSet::new(),
            trace_scroll_handles: HashMap::new(),
            trace_started_at: HashMap::new(),
            trace_elapsed_ms: HashMap::new(),
            turn_started_at: HashMap::new(),
            turn_elapsed_ms: HashMap::new(),
            trace_ticking: false,
            toggled_todo_cards: HashSet::new(),
            arg_stream: Vec::new(),
            output_tail: Vec::new(),
            interactions: Vec::new(),
            subagents: Vec::new(),
            goal: None,
            pinned_compaction: None,
            driver_client_instance_id: None,
            driver_ordinal: 0,
            blobs: BlobCache::default(),
            transcript_before_ord: None,
            transcript_loading: false,
            handoff_seal: None,
            handoff_open: None,
            handoff_auto_jump: None,
            handoff_ticking: false,
            handoff_anchors: Default::default(),
            handoff_from_session_id: None,
            session_created_at_ms: 0,
            handoff_incoming: None,
            handoff_stay: false,
            handoff_eager: false,
            handoff_doc_open: HashSet::new(),
            handoff_open_expanded: false,
            handoff_open_raw: false,
            handoff_seal_rituals: std::collections::HashMap::new(),
            handoff_ritual_done: HashSet::new(),
            handoff_open_ritual: None,
            handoff_open_ritual_done: false,
provider_retry: None,
            exec_sessions: Vec::new(),
            exec_output_tail: Vec::new(),
            stream: StreamState::Connecting,
            md_extensions: crate::markdown::extensions(cx.weak_entity()),
            tok_samples: std::collections::VecDeque::new(),
            timeline_list,
            terminal_panel: crate::terminal_panel::TerminalPanel::new(cx),
            timeline_follow_setting: None,
            settings: None,
            settings_pending: false,
            tool_mode: ToolMode::Standard,
            catalog: None,
            siwc_ready: false,
            dsaccount_signed_in: false,
            composer,
            ask_drafts: Vec::new(),
            ask_hint: None,
            goal_draft: None,
            confirm_truncate_turn: None,
            todos: Vec::new(),
            todo_card_open: true,
            model_params_open: false,
            model_params_max_output: None,
            model_params_custom_tools: None,
            requests,
        }
    }

    pub fn provider_account_unauthenticated(&self, provider_id: &str) -> bool {
        self.catalog
            .as_ref()
            .and_then(|catalog| catalog.providers.get(provider_id))
            .is_some_and(|provider| match qaqh_gpui_wire::profile_vocab::credential_kind(provider) {
                qaqh_core_vocab::CredentialSource::ApiKey => false,
                qaqh_core_vocab::CredentialSource::ChatgptPlan => !self.siwc_ready,
                qaqh_core_vocab::CredentialSource::DeepseekAccount => !self.dsaccount_signed_in,
            })
    }

    /// 发送受阻原因(与 composer 提示同一来源)。
    pub fn send_block_reason(&self) -> Option<&'static str> {
        if !matches!(self.stream, StreamState::Open) {
            return Some("等待连接恢复");
        }
        if self.snapshot_pending { return Some("会话状态正在同步"); }
        if self.handoff_seal.is_some() { return Some("当前会话已封箱，请在后继会话继续"); }
        if self.attachment_pending { return Some("附件正在准备"); }
        if self.outbox.as_ref().is_some_and(|o| matches!(o.state,
            qaqh_gpui_wire::delivery::DeliveryState::Sending | qaqh_gpui_wire::delivery::DeliveryState::Accepted | qaqh_gpui_wire::delivery::DeliveryState::Uncertain)) {
            return Some("上一条消息尚未确认，请先重试或撤回");
        }
        if !self.interactions.is_empty() {
            return Some("有待应答的交互");
        }
        if self.activity == ActivityState::Running {
            return Some("回合运行中,可先中断");
        }
        None
    }

    pub fn request_review_content(
        &self,
        turn: TurnId,
        request: usize,
        references: Vec<ContentRef>,
    ) {
        let _ = self.requests.unbounded_send(BridgeRequest::ReviewContent {
            session_id: self.session_id.clone(),
            turn,
            request,
            references,
        });
    }

    /// 发送 composer 内容;受阻返回原因,空文本静默忽略。
    pub fn composer_send(&mut self, cx: &mut Context<Self>) -> Option<&'static str> {
        if let Some(reason) = self.send_block_reason() {
            return Some(reason);
        }
        let text = self.composer.read(cx).value().trim().to_string();
        if text.is_empty() && self.images.is_empty() && self.attachments.is_empty() {
            return None;
        }
        let command_id = qaqh_gpui_wire::ids::new_id("cmd");
        let input_id = match InputId::new(qaqh_gpui_wire::ids::new_id("inp")) {
            Ok(input_id) => input_id,
            Err(error) => {
                self.operation_error = Some(format!("输入 id 生成失败：{error}"));
                return Some("消息未能发送");
            }
        };
        self.pending_send = Some(PendingSend {
            input_id,
            text,
            attachments: std::mem::take(&mut self.attachments),
            images: std::mem::take(&mut self.images),
        });
        self.outbox = Some(qaqh_gpui_wire::delivery::Outbox::new(command_id));
        if let Err(error) = self.persist_draft(cx) {
            self.operation_error = Some(format!("发送前保存失败：{error}"));
            self.outbox = None;
            self.pending_send = None;
            return Some("草稿未能保存，消息尚未发送");
        }
        self.retry_message();
        None
    }

    pub fn draft(&self, cx: &App) -> crate::persist::Draft {
        crate::persist::Draft {
            text: self.composer.read(cx).value().to_string(),
            images: self.images.clone(),
            attachments: self.attachments.clone(),
            pending_send: self.pending_send.clone(),
            outbox: self.outbox.clone(),
            local_root: self.code_panel.local_root(),
        }
    }
    pub fn persist_draft(&self, cx: &App) -> Result<(), String> {
        let mut state = crate::persist::load()?;
        state.drafts.insert(self.session_id.as_str().to_owned(), self.draft(cx));
        crate::persist::save(&state)
    }
    pub fn retry_message(&mut self) {
        let (Some(outbox), Some(pending)) = (&mut self.outbox, &self.pending_send) else {
            return;
        };
        outbox.state = qaqh_gpui_wire::delivery::DeliveryState::Sending;
        let request = qaqh_ringing_v3::CommandRequest::SendPrompt {
            session_id: self.session_id.clone(),
            input_id: pending.input_id.clone(),
            text: pending.text.clone(),
            attachments: pending.attachments.clone(),
            images: pending.images.clone(),
        };
        if self
            .requests
            .unbounded_send(BridgeRequest::Command {
                command_id: outbox.command_id.clone(),
                request,
            })
            .is_err()
        {
            outbox.state = qaqh_gpui_wire::delivery::DeliveryState::Uncertain;
            outbox.detail = "连接已关闭，请重新连接".into();
        }
    }

    /// 被拒/失败的消息恢复为草稿:outbox 清空,文本/图片/附件回填 composer。
    pub fn restore_outbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.outbox = None;
        if let Some(pending) = self.pending_send.take() {
            self.composer.update(cx, |composer, cx| {
                composer.set_value(pending.text.as_str(), window, cx)
            });
            self.images = pending.images;
            self.attachments = pending.attachments;
        }
    }

    /// 交互处置(V3:审批/Ask 走同一条 `RespondInteraction` 命令)。
    pub fn respond_interaction(
        &mut self,
        interaction_id: &InteractionId,
        resolution: qaqh_ringing_v3::CommandInteractionResolution,
    ) {
        let _ = self.requests.unbounded_send(BridgeRequest::Command {
            command_id: qaqh_gpui_wire::ids::new_id("cmd"),
            request: qaqh_ringing_v3::CommandRequest::RespondInteraction {
                session_id: self.session_id.clone(),
                interaction_id: interaction_id.clone(),
                resolution,
            },
        });
    }

    /// 撤回并重编辑(conversation.truncate):截断含指定回合及其后的所有历史,并回填输入框。
    pub fn truncate_turn(
        &mut self,
        turn_id: TurnId,
        prefill_text: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_running() {
            self.operation_error = Some("当前会话正在运行中，请先停止后再撤回回合。".into());
            self.confirm_truncate_turn = None;
            cx.notify();
            return;
        }
        let _ = self.requests.unbounded_send(BridgeRequest::Session(
            SessionRequest::Truncate {
                session_id: self.session_id.clone(),
                turn_id,
            },
        ));
        if let Some(text) = prefill_text {
            self.composer.update(cx, |composer, cx| {
                composer.set_value(text.as_str(), window, cx);
                composer.focus(window, cx);
            });
        }
        self.confirm_truncate_turn = None;
        cx.notify();
    }

    /// goal 控制(ADR-0015):`SetGoal` 一次给齐正文/状态/预算(V3 语义)。
    pub fn goal_set(&mut self, objective: String, status: GoalStatus, token_budget: Option<u64>) {
        let _ = self.requests.unbounded_send(BridgeRequest::Session(
            SessionRequest::SetGoal {
                session_id: self.session_id.clone(),
                objective,
                status: Some(status),
                token_budget,
            },
        ));
    }

    /// 状态迁移:沿用当前目标正文,只改状态(`SetGoal` 是全量写)。
    pub fn goal_transition(&mut self, status: GoalStatus) {
        let Some(goal) = self.goal.clone() else { return };
        self.goal_set(goal.objective, status, goal.token_budget);
    }

    pub fn goal_clear(&mut self) {
        let _ = self.requests.unbounded_send(BridgeRequest::Session(
            SessionRequest::ClearGoal {
                session_id: self.session_id.clone(),
            },
        ));
    }
    fn new_ask_draft(
        interaction_id: &InteractionId,
        question_ids: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AskDraft {
        let mut customs = Vec::with_capacity(question_ids.len());
        let mut subscriptions = Vec::with_capacity(question_ids.len());
        for q_idx in 0..question_ids.len() {
            let input =
                cx.new(|cx| InputState::new(window, cx).placeholder("其它（输入自定义回答）"));
            let iid_sub = interaction_id.clone();
            let sub = cx.subscribe(
                &input,
                move |store: &mut SessionStore, field, event: &InputEvent, cx| match event {
                    InputEvent::Change => {
                        let has_text = !field.read(cx).value().trim().is_empty();
                        if has_text {
                            if let Some((_, draft)) = store
                                .ask_drafts
                                .iter_mut()
                                .find(|(id, _)| id == &iid_sub)
                                && let Some(slot) = draft.answers.get_mut(q_idx)
                            {
                                *slot = None;
                            }
                            store.ask_hint = None;
                        }
                        cx.notify();
                    }
                    InputEvent::Focus | InputEvent::Blur => cx.notify(),
                    InputEvent::PressEnter { shift: false, .. } => {
                        store.ask_advance_or_submit(&iid_sub, cx);
                    }
                    InputEvent::PressEnter { shift: true, .. } => {}
                },
            );
            customs.push(input);
            subscriptions.push(sub);
        }
        AskDraft {
            question_ids: question_ids.to_vec(),
            answers: vec![None; question_ids.len()],
            customs,
            active_index: 0,
            collapsed: false,
            focus_handle: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    /// 取(或建)ask 卡草稿;仅当题号列表变化时重建,防跨题点击清空已答内容。
    pub fn ask_draft(
        &mut self,
        interaction_id: &InteractionId,
        question_ids: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &mut AskDraft {
        let position = self
            .ask_drafts
            .iter()
            .position(|(id, _)| id == interaction_id);
        let index = match position {
            Some(index) => {
                if self.ask_drafts[index].1.question_ids != question_ids {
                    self.ask_drafts[index].1 =
                        Self::new_ask_draft(interaction_id, question_ids, window, cx);
                }
                index
            }
            None => {
                let draft = Self::new_ask_draft(interaction_id, question_ids, window, cx);
                self.ask_drafts.push((interaction_id.clone(), draft));
                self.ask_drafts.len() - 1
            }
        };
        &mut self.ask_drafts[index].1
    }

    pub fn ask_select_option(
        &mut self,
        interaction_id: &InteractionId,
        index: usize,
        option: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, draft)) = self
            .ask_drafts
            .iter_mut()
            .find(|(id, _)| id == interaction_id)
        else {
            return;
        };
        if let Some(slot) = draft.answers.get_mut(index) {
            *slot = Some(option);
        }
        let field = draft.customs.get(index).cloned();
        let focus = draft.focus_handle.clone();
        self.ask_hint = None;
        if let Some(field) = field {
            field.update(cx, |custom, cx| custom.set_value("", window, cx));
        }
        focus.focus(window, cx);
        cx.notify();
    }

    pub fn ask_focus_custom(
        &mut self,
        interaction_id: &InteractionId,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, draft)) = self
            .ask_drafts
            .iter_mut()
            .find(|(id, _)| id == interaction_id)
        else {
            return;
        };
        if let Some(slot) = draft.answers.get_mut(index) {
            *slot = None;
        }
        let field = draft.customs.get(index).cloned();
        self.ask_hint = None;
        if let Some(field) = field {
            let input_focus = field.read(cx).focus_handle(cx);
            input_focus.focus(window, cx);
        }
        cx.notify();
    }

    pub fn ask_set_step(
        &mut self,
        interaction_id: &InteractionId,
        step: usize,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, draft)) = self
            .ask_drafts
            .iter_mut()
            .find(|(id, _)| id == interaction_id)
            && !draft.question_ids.is_empty()
        {
            draft.active_index = step.min(draft.question_ids.len() - 1);
            self.ask_hint = None;
            cx.notify();
        }
    }

    pub fn ask_toggle_collapsed(
        &mut self,
        interaction_id: &InteractionId,
        cx: &mut Context<Self>,
    ) {
        if let Some((_, draft)) = self
            .ask_drafts
            .iter_mut()
            .find(|(id, _)| id == interaction_id)
        {
            draft.collapsed = !draft.collapsed;
            cx.notify();
        }
    }

    pub fn ask_question_answered(
        &self,
        interaction_id: &InteractionId,
        index: usize,
        cx: &Context<Self>,
    ) -> bool {
        let Some((_, draft)) = self.ask_drafts.iter().find(|(id, _)| id == interaction_id) else {
            return false;
        };
        let has_custom = draft
            .customs
            .get(index)
            .is_some_and(|field| !field.read(cx).value().trim().is_empty());
        has_custom || draft.answers.get(index).is_some_and(Option::is_some)
    }

    pub fn ask_advance_or_submit(
        &mut self,
        interaction_id: &InteractionId,
        cx: &mut Context<Self>,
    ) {
        let Some((active, total)) = self
            .ask_drafts
            .iter()
            .find(|(id, _)| id == interaction_id)
            .map(|(_, draft)| (draft.active_index, draft.question_ids.len()))
        else {
            return;
        };
        if total == 0 {
            return;
        }
        if !self.ask_question_answered(interaction_id, active, cx) {
            self.ask_hint = Some("请先选择选项或输入自定义回答".into());
            cx.notify();
            return;
        }
        if active + 1 < total {
            if let Some((_, draft)) = self
                .ask_drafts
                .iter_mut()
                .find(|(id, _)| id == interaction_id)
            {
                draft.active_index = active + 1;
            }
            self.ask_hint = None;
            cx.notify();
            return;
        }
        if let Some(missing) =
            (0..total).find(|&idx| !self.ask_question_answered(interaction_id, idx, cx))
        {
            if let Some((_, draft)) = self
                .ask_drafts
                .iter_mut()
                .find(|(id, _)| id == interaction_id)
            {
                draft.active_index = missing;
            }
            self.ask_hint = Some(format!("第 {} 题尚未回答", missing + 1));
            cx.notify();
            return;
        }
        let _ = self.ask_submit(interaction_id, cx);
        cx.notify();
    }

    /// 组装应答并发送;未回答的题返回 None(调用方提示)。
    pub fn ask_submit(
        &mut self,
        interaction_id: &InteractionId,
        cx: &Context<Self>,
    ) -> Option<Vec<AskAnswer>> {
        let index = self
            .ask_drafts
            .iter()
            .position(|(id, _)| id == interaction_id)?;
        let draft = &self.ask_drafts[index].1;
        let mut answers = Vec::with_capacity(draft.question_ids.len());
        for (position, id) in draft.question_ids.iter().enumerate() {
            let custom = draft
                .customs
                .get(position)
                .map(|field| field.read(cx).value().trim().to_string())
                .unwrap_or_default();
            let answer = if !custom.is_empty() {
                custom
            } else {
                draft.answers.get(position).cloned().flatten()?
            };
            answers.push(AskAnswer {
                question_id: id.clone(),
                answer,
            });
        }
        self.respond_interaction(
            interaction_id,
            qaqh_ringing_v3::CommandInteractionResolution::Answer {
                answers: answers.clone(),
            },
        );
        self.ask_hint = None;
        Some(answers)
    }

    pub fn ask_dismiss(&mut self, interaction_id: &InteractionId) {
        self.ask_hint = None;
        self.respond_interaction(
            interaction_id,
            qaqh_ringing_v3::CommandInteractionResolution::Dismiss,
        );
    }

    pub fn exec_control(&mut self, exec_id: ExecSessionId, terminate: bool) {
        let request = if terminate {
            qaqh_ringing_v3::CommandRequest::ExecTerminate {
                exec_session_id: exec_id,
            }
        } else {
            qaqh_ringing_v3::CommandRequest::ExecInterrupt {
                exec_session_id: exec_id,
            }
        };
        let _ = self.requests.unbounded_send(BridgeRequest::Command {
            command_id: qaqh_gpui_wire::ids::new_id("cmd"),
            request,
        });
    }

    /// handoff fork(v3.1a):从本会话的 handoff 工具事实创建交接会话。
    /// 载荷抄本会话 meta 的 cwd/model_selection(electron forkHandoffSession 同口径),
    /// title = work_summary,首消息 = 交接文档,handoff_from = 本会话(goal 工作链接缝)。
    pub fn fork_handoff(&mut self, summary: Option<String>, doc: String, cx: &mut Context<Self>) {
        // V3 的 `Create` 不带首消息:交接文档作为新会话的第一条 `SendPrompt`,
        // 由壳层在 `Created` 回执后投递(`SessionNavEvent::HandoffFork`)。
        cx.emit(SessionNavEvent::HandoffFork {
            title: summary,
            doc,
        });
    }

    /// ADR-0016 投影切换:跳后继会话(先通知壳层播飞箱，落位后再 Select)。
    pub fn open_handoff_successor(&mut self, cx: &mut Context<Self>) {
        if let Some(seal) = &self.handoff_seal {
            let Ok(session_id) = SessionId::new(seal.successor_session_id.clone()) else {
                return;
            };
            cx.emit(SessionNavEvent::OpenSession {
                session_id,
                fly_from_session: Some(self.session_id.clone()),
            });
        }
    }

    /// 跨会话导航(例如 B 侧「去源头」跳回 A 会话)。
    pub fn open_session(&mut self, session_id: SessionId, fly: bool, cx: &mut Context<Self>) {
        cx.emit(SessionNavEvent::OpenSession {
            session_id,
            fly_from_session: fly.then(|| self.session_id.clone()),
        });
    }

    /// 跳转到外壳「接入」页完成账号授权。
    pub fn open_access(&self, cx: &mut Context<Self>) {
        cx.emit(SessionNavEvent::OpenAccess);
    }

    /// 「不拽我」:停掉阅读窗自动倒计时，HandoffSealed 落地后不再自动跳走。
    pub fn cancel_handoff_auto(&mut self) {
        self.handoff_stay = true;
        self.handoff_eager = false;
        self.handoff_auto_jump = None;
    }

    /// 立即继续:记录急迫标记，HandoffSealed 一落盘立即飞箱跳到后继。
    pub fn arm_handoff_eager(&mut self) {
        self.handoff_stay = false;
        self.handoff_eager = true;
    }

    pub fn cancel_turn(&mut self) {
        let _ = self.requests.unbounded_send(BridgeRequest::Command {
            command_id: qaqh_gpui_wire::ids::new_id("cmd"),
            request: qaqh_ringing_v3::CommandRequest::CancelTurn {
                session_id: self.session_id.clone(),
            },
        });
    }

    /// 取消看门狗:after_ms 内无终态则桥线程通知 UI 本地收口 + 重灌快照。
    pub fn arm_cancel_watchdog(&self, after_ms: u64) {
        let _ = self.requests.unbounded_send(BridgeRequest::Watchdog {
            label: self.session_id.as_str().to_owned(),
            after_ms,
        });
    }

    /// 记录一次流式净增字符(替换语义下 delta 可为负,负数不入样本)。
    pub fn note_stream_chars(&mut self, delta: i64) {
        if delta <= 0 {
            return;
        }
        let now = std::time::Instant::now();
        self.tok_samples.push_back((now, delta));
        // 3s 滑窗:过期样本出队(electron trackOutputDelta 同口径)。
        while let Some(&(instant, _)) = self.tok_samples.front() {
            if now.duration_since(instant).as_secs_f64() <= 3.0 {
                break;
            }
            self.tok_samples.pop_front();
        }
    }

    /// 当前估算吐字速率 tok/s(字符/4);样本不足(窗口内 < 0.5s)时返回 None。
    pub fn tok_rate(&self) -> Option<f64> {
        let (mut chars, mut first, mut last) = (0i64, None, None);
        for &(instant, delta) in &self.tok_samples {
            chars += delta;
            first = Some(first.unwrap_or(instant));
            last = Some(instant);
        }
        let (first, last) = (first?, last?);
        let span = last.duration_since(first).as_secs_f64();
        if span < 0.5 || chars <= 0 {
            return None;
        }
        Some(chars as f64 / 4.0 / span)
    }

    /// 侧栏终态着色:本会话最近一回合的权威结果。Finished 无错 = ok(true),
    /// Interrupted 或带错 = bad(false);无终态回合 = None(保持 run-status 点)。
    pub fn last_turn_tone(&self) -> Option<bool> {
        let terminal = &self.turns.iter().rev().find(|turn| turn.terminal.is_some())?.terminal;
        terminal.as_ref().map(|terminal| terminal.is_success())
    }

    pub fn patch_settings(&mut self, patch: SessionSettingsPatch) {
        if let Some(settings) = &self.settings
            && !self.settings_pending
            && self
                .requests
                .unbounded_send(BridgeRequest::Session(SessionRequest::PatchSettings {
                    session_id: self.session_id.clone(),
                    patch,
                    expected_revision: settings.settings_revision,
                }))
                .is_ok()
        {
            self.settings_pending = true;
        }
    }

    /// 当前会话实际生效的模型选择（会话显式覆盖优先，未设置时回退全局默认）。
    pub fn effective_model_selection(&self) -> Option<ModelSelection> {
        self.settings
            .as_ref()
            .and_then(|settings| settings.model_selection.clone())
            .or_else(|| {
                self.catalog
                    .as_deref()
                    .and_then(|catalog| catalog.default_model_selection.as_ref())
                    .map(|m| selection_from_profile(m))
            })
    }

    pub fn patch_effort(&mut self, effort: Option<String>) {
        if let Some(mut model) = self.effective_model_selection() {
            model.options.reasoning_effort = effort;
            self.patch_model(model);
        }
    }

    /// 工具模式切换(三档:Core/Standard/Custom;V3 不认压测档)。
    pub fn patch_tool_mode(&mut self, mode: ToolMode) {
        let _ = self.requests.unbounded_send(BridgeRequest::Session(
            SessionRequest::SetToolMode {
                session_id: self.session_id.clone(),
                mode,
            },
        ));
    }

    pub fn patch_model(&mut self, model: ModelSelection) {
        self.patch_settings(SessionSettingsPatch {
            model_selection: Some(SettingOverride::Use(model)),
            ..Default::default()
        });
    }

    pub fn open_model_params(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let max_out_val = self
            .effective_model_selection()
            .and_then(|m| m.options.max_output_tokens)
            .map(|n| n.to_string())
            .unwrap_or_default();
        let custom_tools_val = self
            .settings
            .as_ref()
            .map(|s| s.custom_tools.join(", "))
            .unwrap_or_default();

        let max_out_input = match &self.model_params_max_output {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("模型默认"));
                self.model_params_max_output = Some(input.clone());
                input
            }
        };
        max_out_input.update(cx, |state, cx| state.set_value(&max_out_val, window, cx));

        let custom_tools_input = match &self.model_params_custom_tools {
            Some(input) => input.clone(),
            None => {
                let input = cx.new(|cx| InputState::new(window, cx).placeholder("英文逗号分隔"));
                self.model_params_custom_tools = Some(input.clone());
                input
            }
        };
        custom_tools_input.update(cx, |state, cx| state.set_value(&custom_tools_val, window, cx));

        self.model_params_open = true;
        cx.notify();
    }

    pub fn patch_max_output_tokens(&mut self, max_output_tokens: Option<u32>) {
        if let Some(mut model) = self.effective_model_selection() {
            model.options.max_output_tokens = max_output_tokens;
            self.patch_model(model);
        }
    }

    pub fn patch_native_web_search(&mut self, native_web_search: Option<bool>) {
        if let Some(mut model) = self.effective_model_selection() {
            model.options.native_web_search = native_web_search;
            self.patch_model(model);
        }
    }

    pub fn patch_custom_tools(&mut self, names: Vec<String>) {
        self.patch_settings(SessionSettingsPatch {
            tools: Some(qaqh_ringing_v3::ToolsSettingPatch::Custom { tools: names }),
            ..Default::default()
        });
    }

    /// 移入目标工作区分组。
    pub fn move_to_workspace(&mut self, workspace_id: WorkspaceId) {
        let _ = self.requests.unbounded_send(BridgeRequest::Workspace(
            qaqh_ringing_v3::WorkspaceRequest::MoveSession {
                session_id: self.session_id.clone(),
                workspace_id,
            },
        ));
    }

    /// 工作区目录尾段(供 TitleBar 面包屑与无标题降级)。
    pub fn workspace_tail(&self) -> Option<gpui_kit::SharedString> {
        self.settings
            .as_ref()
            .and_then(|entry| entry.cwd.as_deref())
            .filter(|cwd| !cwd.is_empty())
            .map(|cwd| {
                let trimmed = cwd.trim_end_matches(['/', '\\']);
                let tail = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
                gpui_kit::SharedString::from(tail.to_owned())
            })
    }

    /// 清洗后展示标题:拦截空串/"(无标题)"/"ses_*" ULID,避免内部 ID 泄露到标题位。
    pub fn display_title(&self) -> gpui_kit::SharedString {
        if let Some(title) = self.title.as_deref().map(str::trim)
            && !title.is_empty()
            && title != "(无标题)"
            && !title.starts_with("ses_")
        {
            return gpui_kit::SharedString::from(title.to_owned());
        }
        self.workspace_tail()
            .unwrap_or_else(|| gpui_kit::SharedString::from("未命名会话"))
    }

    pub fn is_running(&self) -> bool {
        self.activity == ActivityState::Running
    }

    /// 向上翻页:以快照/上一页给出的最早 `ord` 为游标,请求更早的一页。
    ///
    /// `timeline_epoch` 由快照给出;失配时服务端回 `StaleCursor`,前端一律重取快照
    /// (禁止拼接——拼出来的历史是错位的假历史)。
    pub fn request_older(&mut self) {
        if self.transcript_loading {
            return;
        }
        let Some(before_ord) = self.transcript_before_ord else {
            return;
        };
        if before_ord == 0 {
            self.transcript_before_ord = None;
            return;
        }
        self.transcript_loading = true;
        let _ = self.requests.unbounded_send(BridgeRequest::LoadTranscript {
            session_id: self.session_id.clone(),
            before_ord,
            limit: TAIL_PAGE,
            timeline_epoch: self.timeline_epoch,
        });
    }

    /// 内容引用 → 可渲染文本。
    ///
    /// `Inline` 直接给正文;`Windowed` 只给有界尾窗(权威全文另有出处);
    /// `Blob` 未到货时给占位符并保持原样——到货后 `BridgeMsg::Blob` 会触发重渲染。
    pub fn content_text(&self, content: &ContentRef) -> String {
        match content {
            ContentRef::Inline { text } => text.clone(),
            ContentRef::Windowed {
                preview,
                total_bytes,
                ..
            } => format!("{preview}\n…（尾窗预览，共 {total_bytes} 字节）"),
            ContentRef::Blob { hash, .. } => match self.blobs.get(hash) {
                Some(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                None => crate::timeline::CONTENT_LOADING.to_owned(),
            },
        }
    }

    /// 扫描块/工具里的大内容引用,对首次见到的哈希发一次 `FetchBlob`。
    /// 快照、增量块、工具视图更新后调用;重复调用无副作用(幂等)。
    pub fn prefetch_blobs(&mut self) {
        let mut wanted: Vec<qaqh_core_vocab::ContentHash> = Vec::new();
        for block in &self.entries {
            if let ContentRef::Blob { hash, .. } = &block.content {
                wanted.push(*hash);
            }
            for attachment in &block.attachments {
                if let ContentRef::Blob { hash, .. } = attachment {
                    wanted.push(*hash);
                }
            }
            wanted.extend(block.images.iter().map(|image| image.hash));
        }
        for tool in &self.tools {
            for content in [&tool.args, &tool.output, &tool.diff].into_iter().flatten() {
                if let ContentRef::Blob { hash, .. } = content {
                    wanted.push(*hash);
                }
            }
        }
        for hash in wanted {
            if self.blobs.want(&hash) {
                let _ = self.requests.unbounded_send(BridgeRequest::FetchBlob(hash));
            }
        }
    }

    /// 当前会话的上下文占用与请求缓存命中统计(直读 V3 块/回合 usage 与模型目录窗口,零转换层)。
    pub fn telemetry(&self) -> Option<SessionTelemetry> {
        let last_block = self.entries.iter().rev().find_map(|entry| match entry {
            BlockView {
                turn_id: Some(turn_id),
                kind: BlockKind::Answer | BlockKind::Reasoning,
                model: Some(model),
                usage: Some(usage),
                ..
            } => Some((turn_id, model.as_str(), usage)),
            _ => None,
        });
        let last_turn = self
            .turns
            .iter()
            .rev()
            .find_map(|turn| turn.usage.as_ref().map(|usage| (&turn.turn_id, usage)));

        let turn_pos = |id: &TurnId| self.turns.iter().position(|turn| &turn.turn_id == id);
        let (latest_usage, latest_model): (&UsageInfo, Option<&str>) =
            match (last_block, last_turn) {
                (Some((b_turn, b_model, b_usage)), Some((t_turn, t_usage))) => {
                    match (turn_pos(b_turn), turn_pos(t_turn)) {
                        (Some(bi), Some(ti)) if ti > bi => (t_usage, None),
                        _ => (b_usage, Some(b_model)),
                    }
                }
                (Some((_, b_model, b_usage)), None) => (b_usage, Some(b_model)),
                (None, Some((_, t_usage))) => (t_usage, None),
                (None, None) => return None,
            };

        let context_window = self.catalog.as_deref().and_then(|catalog| {
            let from_settings = self
                .settings
                .as_ref()
                .and_then(|s| s.model_selection.as_ref())
                .and_then(|sel| {
                    catalog
                        .providers
                        .get(&sel.provider_id)
                        .and_then(|provider| provider.models.get(&sel.model_id))
                        .map(|model| model.context_window)
                })
                .filter(|&window| window > 0);
            from_settings.or_else(|| {
                let model_id = latest_model.filter(|id| !id.is_empty())?;
                catalog
                    .providers
                    .values()
                    .find_map(|provider| provider.models.get(model_id))
                    .map(|model| model.context_window)
                    .filter(|&window| window > 0)
            })
        });

        let used_tokens = usage_total_input(latest_usage);
        let used_pct = context_window
            .filter(|&window| window > 0)
            .map(|window| {
                ((used_tokens as f64 / window as f64) * 100.0)
                    .round()
                    .clamp(0.0, 100.0) as u8
            });
        let latest_cache = usage_cache_stats(latest_usage);

        // 同一回合末轮的 usage 会同时落到末个助手块与回合摘要上,按 turn_id 去重。
        let mut turns_with_block_usage: HashSet<&TurnId> = HashSet::new();
        let mut usage_requests: usize = 0;
        let mut cum_hit: u64 = 0;
        let mut cum_total: u64 = 0;
        for entry in &self.entries {
            if let BlockView {
                turn_id: Some(turn_id),
                kind: BlockKind::Answer | BlockKind::Reasoning,
                usage: Some(usage),
                ..
            } = entry
            {
                turns_with_block_usage.insert(turn_id);
                usage_requests += 1;
                if let Some(stats) = usage_cache_stats(usage) {
                    cum_hit = cum_hit.saturating_add(stats.hit_tokens);
                    cum_total = cum_total.saturating_add(stats.total_tokens);
                }
            }
        }
        for turn in &self.turns {
            if !turns_with_block_usage.contains(&turn.turn_id)
                && let Some(usage) = &turn.usage
            {
                usage_requests += 1;
                if let Some(stats) = usage_cache_stats(usage) {
                    cum_hit = cum_hit.saturating_add(stats.hit_tokens);
                    cum_total = cum_total.saturating_add(stats.total_tokens);
                }
            }
        }
        let cumulative_cache = (cum_total > 0).then(|| CacheHitStats {
            hit_tokens: cum_hit,
            total_tokens: cum_total,
            hit_pct: ((cum_hit as f64 / cum_total as f64) * 100.0)
                .round()
                .clamp(0.0, 100.0) as u8,
        });

        Some(SessionTelemetry {
            used_tokens,
            context_window,
            used_pct,
            input_tokens: latest_usage.input_tokens,
            output_tokens: latest_usage.output_tokens,
            reasoning_tokens: latest_usage.reasoning_tokens.unwrap_or_default(),
            latest_cache,
            cumulative_cache,
            usage_requests,
        })
    }

}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheHitStats {
    pub hit_tokens: u64,
    pub total_tokens: u64,
    pub hit_pct: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTelemetry {
    pub used_tokens: u64,
    pub context_window: Option<u32>,
    pub used_pct: Option<u8>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    pub latest_cache: Option<CacheHitStats>,
    pub cumulative_cache: Option<CacheHitStats>,
    pub usage_requests: usize,
}

/// 单次请求输入上下文总量(V3 `UsageInfo.input_tokens` 即权威口径)。
pub fn usage_total_input(usage: &UsageInfo) -> u64 {
    usage.input_tokens
}

/// 缓存命中率统计(仅在服务商明确上报 `cached_input_tokens` 时返回 `Some`,
/// 从而区分「0% 命中」与「未上报」)。
pub fn usage_cache_stats(usage: &UsageInfo) -> Option<CacheHitStats> {
    let hit = usage.cached_input_tokens?;
    let denom = usage.input_tokens;
    if denom == 0 {
        return None;
    }
    let hit_pct = ((hit as f64 / denom as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    Some(CacheHitStats {
        hit_tokens: hit,
        total_tokens: denom,
        hit_pct,
    })
}

/// 会话模型选项(目录里能被解析的模型;会话绑定本身用 V3 `ModelSelection`)。
pub fn model_options_from(catalog: &ProfilesDocument) -> Vec<ModelSelection> {
    catalog
        .providers
        .iter()
        .flat_map(|(provider_id, provider)| {
            provider
                .models
                .keys()
                .map(move |model_id| ModelSelection {
                    provider_id: provider_id.clone(),
                    model_id: model_id.clone(),
                    options: Default::default(),
                })
        })
        .filter(|model| {
            qaqh_profile::resolve_model_selection(catalog, &profile_selection(model)).is_ok()
        })
        .collect()
}

/// 配置文档的模型选择 → V3 会话绑定(同名字段跨 crate 的唯二接缝之一)。
pub fn selection_from_profile(selection: &qaqh_profile::ModelSelection) -> ModelSelection {
    ModelSelection {
        provider_id: selection.provider_id.clone(),
        model_id: selection.model_id.clone(),
        options: qaqh_core_vocab::ModelSelectionOptions {
            max_output_tokens: selection.options.max_output_tokens,
            reasoning_effort: selection.options.reasoning_effort.clone(),
            native_web_search: selection.options.native_web_search,
        },
    }
}

/// V3 会话绑定 → 配置文档的模型选择(目录解析/预设匹配用)。
pub fn profile_selection(selection: &ModelSelection) -> qaqh_profile::ModelSelection {
    qaqh_profile::ModelSelection {
        provider_id: selection.provider_id.clone(),
        model_id: selection.model_id.clone(),
        options: qaqh_profile::model::ModelSelectionOptions {
            max_output_tokens: selection.options.max_output_tokens,
            reasoning_effort: selection.options.reasoning_effort.clone(),
            native_web_search: selection.options.native_web_search,
        },
    }
}

/// `session.list` 侧栏条目:V3 `SessionSummary` 直读,只为分组排序保留需要的字段。
#[derive(Clone)]
pub struct SessionRow {
    pub id: SessionId,
    pub title: String,
    pub created_at_ms: u64,
    /// V3 已修正为毫秒(`SessionSummary.updated_at_ms`)。
    pub updated_at_ms: u64,
    pub activity: ActivityState,
    pub workspace_id: Option<WorkspaceId>,
    pub cwd: Option<String>,
    pub archived: bool,
    pub handoff_from: Option<SessionId>,
}

pub fn rows_from(sessions: &[SessionSummary]) -> Vec<SessionRow> {
    sessions
        .iter()
        .map(|session| SessionRow {
            id: session.session_id.clone(),
            title: session.title.clone().unwrap_or_else(|| "(无标题)".into()),
            created_at_ms: session.created_at_ms,
            updated_at_ms: session.updated_at_ms,
            activity: session.activity,
            workspace_id: session.workspace_id.clone(),
            cwd: session.cwd.clone(),
            archived: session.archived,
            handoff_from: session.handoff_from.clone(),
        })
        .collect()
}

/// 会话任务条目。
#[derive(Clone)]
pub struct TodoRow {
    pub title: String,
    pub description: String,
    pub status: String,
    pub evidence: Option<String>,
}

pub fn todos_from(value: &qaqh_ringing_v3::TodoListSnapshot) -> Vec<TodoRow> {
    value.items.iter()
        .map(|item| TodoRow {
            title: item.title.clone(),
            description: item.description.clone(),
            status: item.status.clone(),
            evidence: item.evidence.clone(),
        })
        .collect()
}

/// 侧栏分组:workspace order 决定组序,组内 updated_at 降序;未分组兜底。
pub struct WorkspaceGroup {
    pub id: Option<WorkspaceId>,
    pub title: String,
    pub rows: Vec<SessionRow>,
}

pub fn groups(sessions: &[SessionRow], workspaces: &[WorkspaceEntry]) -> Vec<WorkspaceGroup> {
    let mut ordered: Vec<(Option<&WorkspaceEntry>, Vec<&SessionRow>)> = workspaces.iter().map(|entry| (Some(entry), Vec::new())).collect();
    for row in sessions {
        let workspace = row
            .workspace_id
            .as_ref()
            .and_then(|id| workspaces.iter().find(|entry| &entry.id == id));
        match ordered
            .iter_mut()
            .find(|(entry, _)| *entry == workspace)
        {
            Some((_, rows)) => rows.push(row),
            None => ordered.push((workspace, vec![row])),
        }
    }
    ordered.sort_by_key(|(entry, _)| entry.map(|entry| entry.order).unwrap_or(u32::MAX));
    ordered
        .into_iter()
        .map(|(entry, rows)| {
            let mut rows: Vec<SessionRow> = rows.into_iter().cloned().collect();
            // 组内新→旧:创建时刻(毫秒)降序,决胜键 session_id 字典序(绝对确定性排序)。
            rows.sort_by(|a, b| {
                b.created_at_ms
                    .cmp(&a.created_at_ms)
                    .then_with(|| a.id.as_str().cmp(b.id.as_str()))
            });
            WorkspaceGroup {
                id: entry.map(|entry| entry.id.clone()),
                title: entry.map(|entry| entry.title.clone()).unwrap_or_else(|| "未分组".into()),
                rows,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_sort_by_created_ms_then_id() {
        let mk = |id: &str, created_at_ms: u64| SessionRow {
            id: SessionId::new(id).expect("id"),
            title: String::new(),
            created_at_ms,
            updated_at_ms: created_at_ms,
            activity: ActivityState::Idle,
            workspace_id: None,
            cwd: None,
            archived: false,
            handoff_from: None,
        };
        let old = mk("ses_b", 100_000);
        let new = mk("ses_a", 200_000);
        let grouped = groups(&[old, new], &[]);
        let ids: Vec<_> = grouped[0]
            .rows
            .iter()
            .map(|row| row.id.as_str().to_owned())
            .collect();
        assert_eq!(ids, ["ses_a", "ses_b"]);
    }

    #[test]
    fn telemetry_usage_input_and_cache_stats_contract() {
        let unreported = UsageInfo {
            input_tokens: 1_200,
            output_tokens: 300,
            cached_input_tokens: None,
            reasoning_tokens: None,
        };
        assert_eq!(usage_total_input(&unreported), 1_200);
        assert_eq!(usage_cache_stats(&unreported), None);

        let zero_hit = UsageInfo {
            input_tokens: 1_000,
            output_tokens: 0,
            cached_input_tokens: Some(0),
            reasoning_tokens: None,
        };
        assert_eq!(usage_total_input(&zero_hit), 1_000);
        assert_eq!(
            usage_cache_stats(&zero_hit),
            Some(CacheHitStats {
                hit_tokens: 0,
                total_tokens: 1_000,
                hit_pct: 0,
            })
        );

        let high_hit = UsageInfo {
            input_tokens: 100_000,
            output_tokens: 0,
            cached_input_tokens: Some(85_000),
            reasoning_tokens: None,
        };
        assert_eq!(usage_total_input(&high_hit), 100_000);
        assert_eq!(
            usage_cache_stats(&high_hit),
            Some(CacheHitStats {
                hit_tokens: 85_000,
                total_tokens: 100_000,
                hit_pct: 85,
            })
        );
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsEditor {
    #[default]
    Local,
    Config,
    Catalog,
    Session,
    Accounts,
    Workspaces,
    Archived,
    About,
}

/// 服务面账号服务(siwc = ChatGPT 套餐 / dsaccount = DeepSeek 账号)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountService {
    Siwc,
    Dsaccount,
}

/// 进行中的浏览器登录流程:轮询 sign_in.status 至终态,过期自动收口。
pub struct AccountSignIn {
    pub service: AccountService,
    /// siwc 是 flow_id;dsaccount 是 attempt_id。
    pub flow_id: String,
    pub expires_at_ms: u64,
}

impl AccountSignIn {
    pub fn poll_request(&self) -> BridgeRequest {
        BridgeRequest::Account(match self.service {
            AccountService::Siwc => AccountRequest::SiwcSignInPoll {
                flow_id: self.flow_id.clone(),
            },
            AccountService::Dsaccount => AccountRequest::DsSignInPoll {
                attempt_id: self.flow_id.clone(),
            },
        })
    }
}

pub fn next_option<'a, T: PartialEq>(options: &'a [T], current: Option<&T>) -> Option<&'a T> {
    let index = current.and_then(|current| options.iter().position(|option| option == current));
    match index {
        Some(index) => options.get(index + 1).or_else(|| options.first()),
        None => options.first(),
    }
}

pub fn next_model<'a>(
    options: &'a [ModelSelection],
    current: Option<&ModelSelection>,
) -> Option<&'a ModelSelection> {
    let current = current.and_then(|current| options.iter().find(|model| {
        model.provider_id == current.provider_id && model.model_id == current.model_id
    }));
    next_option(options, current)
}

pub fn model_effort_levels(catalog: &qaqh_profile::ProfilesDocument, model: &ModelSelection) -> Vec<String> {
    catalog.providers.get(&model.provider_id)
        .and_then(|provider| provider.models.get(&model.model_id))
        .filter(|model| model.capabilities.reasoning)
        .map(|model| model.reasoning_levels.clone()).unwrap_or_default()
}

pub fn model_default_effort(
    catalog: &qaqh_profile::ProfilesDocument,
    model: &ModelSelection,
) -> Option<String> {
    catalog
        .providers
        .get(&model.provider_id)
        .and_then(|provider| provider.models.get(&model.model_id))
        .filter(|model| model.capabilities.reasoning)
        .and_then(|model| model.defaults.reasoning_effort.clone())
        .filter(|effort| !effort.is_empty())
}

