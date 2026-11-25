//! GPUI 实体状态(M3):字段直持 RingingV2 契约类型,零前端 DTO。
//! 多会话:visited store 常驻缓存、事件按 seed 路由;输入框统一走
//! gpui-component 的 InputState/TextareaState(InputState::new 需要
//! &mut Window,故实体构造一律在窗口上下文内完成)。

use std::collections::HashSet;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use gpui_kit::App;
use gpui_kit::AppContext as _;
use gpui_kit::Context;
use gpui_kit::Entity;
use gpui_kit::FocusHandle;
use gpui_kit::Focusable;
use gpui_kit::Subscription;
use gpui_kit::Window;
use gpui_kit::{ListAlignment, ListState, px};
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::BlockId;
use qaqh_ringing_contract::fact::ExecSessionId;
use qaqh_ringing_contract::fact::InteractionId;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::fact::TurnId;
use qaqh_ringing_contract::state::ExecSessionView;
use qaqh_ringing_contract::state::GoalStatus;
use qaqh_ringing_contract::state::GoalView;
use qaqh_ringing_contract::state::InteractionView;
use qaqh_ringing_contract::state::SubagentView;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_ringing_contract::state::TurnView;
use qaqh_ringing_contract::workspace::WorkspaceEntry;

use qaqh_gpui_wire::bridge::BridgeRequest;
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use qaqh_client::{
    CommandTarget, RingingCommand, SessionSettingsPatch, ToolsSettingPatch,
};
use qaqh_profile::ProfilesDocument;
use qaqh_types::{ModelSelection, SessionListEntry, SessionRunStatus, SettingValue};
use qaqh_ringing_contract::state::ToolCallState;

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
        session_id: String,
        fly_from_session: Option<String>,
    },
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
pub fn handoff_facts(tool: &ToolCallView) -> Option<HandoffFacts> {
    if tool.tool_name.as_deref() != Some("handoff") {
        return None;
    }
    let success = matches!(&tool.state, ToolCallState::Finished { outcome } if outcome.is_success());
    if !success {
        return None;
    }
    let args = tool.args.as_ref()?;
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
        .and_then(|output| extract_machine_json(crate::timeline::content_text(output)));
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
        call_id: tool.call_id.clone(),
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

pub struct SessionStore {
    pub snapshot_pending: bool,
    pub buffered_frames: Vec<qaqh_ringing_contract::events::SessionEventFrame>,
    pub outbox: Option<qaqh_gpui_wire::delivery::Outbox>,
    pub images: Vec<qaqh_client::ImageBlock>,
    pub attachments: Vec<qaqh_client::ContentRef>,
    pub attachment_pending: bool,
    pub operation_error: Option<String>,
    pub client_session_id: Option<String>,
    pub output_budget: usize,
    pub search: Entity<InputState>,
    pub search_open: bool,
    pub code_panel: crate::code_panel::CodePanel,
    pub session_id: String,
    pub title: Option<String>,
    pub activity: Option<ActivityState>,
    pub current_turn_id: Option<TurnId>,
    /// 已覆盖水位(互斥上界):快照 seq 与随后每帧 seq+1。
    pub seq: u64,
    pub turns: Vec<TurnView>,
    pub entries: Vec<TimelineEntry>,
    /// 未 seal 的助手流式块:block_id → (kind, 累计替换文本)。
    pub streaming: Vec<(BlockId, AssistantBlockKind, String)>,
    /// 工具调用业务状态(单一权威,与 timeline 的 ToolCall 条目按 call_id 关联)。
    pub tools: Vec<ToolCallView>,
    pub expanded_tools: HashSet<ToolCallId>,
    /// 展开的思考链路块(默认折叠,同 expanded_tools 口径)。
    pub expanded_thinking: HashSet<BlockId>,
    /// 展开的连续工具轨迹组(以组内首条 entry 索引为 key)。
    pub expanded_groups: HashSet<usize>,
    /// 未 declare 的参数生成流:call_id → 流式摘要。
    pub arg_stream: Vec<ArgStream>,
    /// 运行中工具的输出尾窗:call_id → 累计文本。
    pub output_tail: Vec<(ToolCallId, String)>,
    pub interactions: Vec<InteractionView>,
    pub subagents: Vec<SubagentView>,
    /// 会话目标(ADR-0015):bootstrap + goal_updated 事件维护。
    pub goal: Option<GoalView>,
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
    pub handoff_from_session_id: Option<String>,
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
    pub exec_sessions: Vec<ExecSessionView>,
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
    pub settings: Option<SessionListEntry>,
    pub settings_pending: bool,
    pub catalog: Option<Arc<ProfilesDocument>>,
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
        session_id: String,
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
            images: draft.images,
            attachments: draft.attachments,
            attachment_pending: false,
            operation_error: None,
            client_session_id: None,
            output_budget: qaqh_gpui_kit::settings::get(cx).behavior.output_tail_max,
            search,
            search_open: false,
            session_id,
            title: None,
            activity: None,
            current_turn_id: None,
            seq: 0,
            turns: Vec::new(),
            entries: Vec::new(),
            streaming: Vec::new(),
            tools: Vec::new(),
            expanded_tools: HashSet::new(),
            expanded_thinking: HashSet::new(),
            expanded_groups: HashSet::new(),
            arg_stream: Vec::new(),
            output_tail: Vec::new(),
            interactions: Vec::new(),
            subagents: Vec::new(),
            goal: None,
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
            catalog: None,
            composer,
            ask_drafts: Vec::new(),
            ask_hint: None,
            goal_draft: None,
            confirm_truncate_turn: None,
            requests,
        }
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
            return Some("上一条消息尚未确认，请先查回执");
        }
        if self.interactions.iter().any(|i| i.resolution.is_none()) {
            return Some("有待应答的交互");
        }
        if self.activity == Some(ActivityState::Running) {
            return Some("回合运行中,可先中断");
        }
        None
    }

    pub fn request_review_content(
        &self,
        turn: TurnId,
        request: usize,
        references: Vec<qaqh_ringing_contract::fact::ContentRef>,
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
        let outbox = qaqh_gpui_wire::delivery::Outbox {
            command_id: qaqh_ringing_contract::ulid::generate_ulid(),
            message: qaqh_client::ConversationSendMessage {
                text, images: self.images.clone(), attachments: Some(self.attachments.clone()),
                message_id: None, input_purpose: Default::default(), as_system: false,
                inter_agent: None, subagent_terminal: None, widget_context: None,
            },
            state: qaqh_gpui_wire::delivery::DeliveryState::Sending, detail: String::new(),
        };
        self.outbox = Some(outbox);
        if let Err(error) = self.persist_draft(cx) {
            self.operation_error = Some(format!("发送前保存失败：{error}"));
            self.outbox = None;
            return Some("草稿未能保存，消息尚未发送");
        }
        self.retry_message();
        None
    }

    pub fn draft(&self, cx: &App) -> crate::persist::Draft {
        crate::persist::Draft {
            text: self.composer.read(cx).value().to_string(), images: self.images.clone(),
            attachments: self.attachments.clone(), outbox: self.outbox.clone(),
            local_root: self.code_panel.local_root(),
        }
    }
    pub fn persist_draft(&self, cx: &App) -> Result<(), String> {
        let mut state = crate::persist::load()?;
        state.drafts.insert(self.session_id.clone(), self.draft(cx));
        crate::persist::save(&state)
    }
    pub fn retry_message(&mut self) {
        if let Some(outbox) = &mut self.outbox {
            outbox.state = qaqh_gpui_wire::delivery::DeliveryState::Sending;
            if self.requests.unbounded_send(BridgeRequest::SendMessage { seed: self.session_id.clone(), outbox: outbox.clone() }).is_err() {
                outbox.state = qaqh_gpui_wire::delivery::DeliveryState::Uncertain;
                outbox.detail = "连接已关闭，请重新连接".into();
            }
        }
    }
    pub fn check_receipt(&self) {
        if let Some(outbox) = &self.outbox {
            let _ = self.requests.unbounded_send(BridgeRequest::Receipt { seed: self.session_id.clone(), command_id: outbox.command_id.clone() });
        }
    }

    /// 被拒/失败的消息恢复为草稿:outbox 清空,文本/图片/附件回填 composer。
    pub fn restore_outbox(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(outbox) = self.outbox.take() {
            let text = outbox.message.text.clone();
            self.composer.update(cx, |composer, cx| {
                composer.set_value(text.as_str(), window, cx)
            });
            self.images = outbox.message.images;
            self.attachments = outbox.message.attachments.unwrap_or_default();
        }
    }

    pub fn respond_permission(
        &mut self,
        tool_call_id: String,
        approved: bool,
        scope: Option<qaqh_ringing_contract::fact::PermissionGrantScope>,
    ) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RespondPermission {
                seed: self.session_id.clone(),
                tool_call_id,
                approved,
                scope,
            });
    }

    /// 撤回并重编辑(conversation.truncate):截断含指定回合及其后的所有历史,并回填输入框。
    pub fn truncate_turn(
        &mut self,
        turn_id: String,
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
        let _ = self.requests.unbounded_send(BridgeRequest::Command {
            target: CommandTarget::Session {
                session_id: self.session_id.clone(),
            },
            command: RingingCommand::ConversationTruncate { turn_id },
        });
        if let Some(text) = prefill_text {
            self.composer.update(cx, |composer, cx| {
                composer.set_value(text.as_str(), window, cx);
                composer.focus(window, cx);
            });
        }
        self.confirm_truncate_turn = None;
        cx.notify();
    }

    /// goal 控制(ADR-0015):objective=创建/改目标;status=状态迁移;
    /// token_budget 双层语义由命令面表达(这里 None = 不动)。
    pub fn goal_set(&mut self, objective: Option<String>, status: Option<GoalStatus>, token_budget: Option<Option<u64>>) {
        let _ = self.requests.unbounded_send(BridgeRequest::Command {
            target: CommandTarget::Session { session_id: self.session_id.clone() },
            command: RingingCommand::GoalSet { objective, status, token_budget },
        });
    }

    pub fn goal_clear(&mut self) {
        let _ = self.requests.unbounded_send(BridgeRequest::Command {
            target: CommandTarget::Session { session_id: self.session_id.clone() },
            command: RingingCommand::GoalClear {},
        });
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
    ) -> Option<Vec<qaqh_client::AskAnswer>> {
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
            answers.push(qaqh_client::AskAnswer {
                question_id: id.clone(),
                answer,
            });
        }
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RespondAsk {
                seed: self.session_id.clone(),
                interaction_id: interaction_id.0.clone(),
                answers: answers.clone(),
            });
        self.ask_hint = None;
        Some(answers)
    }

    pub fn ask_dismiss(&mut self, interaction_id: &InteractionId) {
        self.ask_hint = None;
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::DismissAsk {
                seed: self.session_id.clone(),
                interaction_id: interaction_id.0.clone(),
            });
    }

    pub fn exec_control(&mut self, exec_id: String, terminate: bool) {
        let _ = self.requests.unbounded_send(BridgeRequest::ExecControl {
            exec_id,
            terminate,
        });
    }

    /// handoff fork(v3.1a):从本会话的 handoff 工具事实创建交接会话。
    /// 载荷抄本会话 meta 的 cwd/model_selection(electron forkHandoffSession 同口径),
    /// title = work_summary,首消息 = 交接文档,handoff_from = 本会话(goal 工作链接缝)。
    pub fn fork_handoff(&mut self, summary: Option<String>, doc: String) {
        let meta = self.settings.as_ref().map(|settings| &settings.meta);
        let _ = self.requests.unbounded_send(BridgeRequest::CreateSession {
            cwd: meta.and_then(|meta| meta.cwd.clone()),
            model_selection: meta.and_then(|meta| meta.model_selection.clone()),
            tool_mode: None,
            title: summary,
            first_message: Some(doc),
            handoff_from: Some(self.session_id.clone()),
        });
    }

    /// ADR-0016 投影切换:跳后继会话(先通知壳层播飞箱，落位后再 Select)。
    pub fn open_handoff_successor(&mut self, cx: &mut Context<Self>) {
        if let Some(seal) = &self.handoff_seal {
            cx.emit(SessionNavEvent::OpenSession {
                session_id: seal.successor_session_id.clone(),
                fly_from_session: Some(self.session_id.clone()),
            });
        }
    }

    /// 跨会话导航(例如 B 侧「去源头」跳回 A 会话)。
    pub fn open_session(&mut self, session_id: String, fly: bool, cx: &mut Context<Self>) {
        cx.emit(SessionNavEvent::OpenSession {
            session_id,
            fly_from_session: fly.then(|| self.session_id.clone()),
        });
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
        let _ = self.requests.unbounded_send(BridgeRequest::Cancel {
            seed: self.session_id.clone(),
            turn_id: self.current_turn_id.as_ref().map(|id| id.0.clone()),
        });
    }

    /// 取消看门狗:after_ms 内无终态则桥线程通知 UI 本地收口 + 重灌快照。
    pub fn arm_cancel_watchdog(&self, after_ms: u64) {
        let _ = self.requests.unbounded_send(BridgeRequest::Watchdog {
            seed: self.session_id.clone(),
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
        let outcome = &self.turns.iter().rev().find(|turn| turn.outcome.is_some())?.outcome;
        outcome.as_ref().map(|outcome| match outcome {
            qaqh_ringing_contract::state::TurnOutcome::Finished { error: None, .. } => true,
            _ => false,
        })
    }

    pub fn patch_settings(&mut self, patch: SessionSettingsPatch) {
        if let Some(settings) = &self.settings
            && !self.settings_pending
            && self.requests.unbounded_send(BridgeRequest::PatchSessionSettings {
                seed: self.session_id.clone(),
                patch,
                revision: settings.meta.settings_revision,
            }).is_ok()
        {
            self.settings_pending = true;
        }
    }

    pub fn patch_effort(&mut self, effort: Option<String>) {
        if let Some(mut model) = self.settings.as_ref().and_then(|settings| settings.meta.model_selection.clone()) {
            model.options.reasoning_effort = effort;
            self.patch_model(model);
        }
    }

    pub fn patch_tools_mode(&mut self, name: String) {
        self.patch_settings(SessionSettingsPatch {
            tools: Some(ToolsSettingPatch::Preset { name }),
            ..Default::default()
        });
    }

    pub fn patch_model(&mut self, model: ModelSelection) {
        self.patch_settings(SessionSettingsPatch {
            model_selection: Some(SettingValue::Use { value: model }),
            ..Default::default()
        });
    }

    /// 移入目标工作区分组。
    pub fn move_to_workspace(&mut self, workspace_id: String) {
        let _ = self.requests.unbounded_send(BridgeRequest::MoveSession {
            seed: self.session_id.clone(),
            workspace_id,
        });
    }

    /// 工作区目录尾段(供 TitleBar 面包屑与无标题降级)。
    pub fn workspace_tail(&self) -> Option<gpui_kit::SharedString> {
        self.settings
            .as_ref()
            .and_then(|entry| entry.meta.cwd.as_deref())
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
        self.activity == Some(ActivityState::Running)
    }

}

pub fn model_options_from(catalog: &ProfilesDocument) -> Vec<ModelSelection> {
    catalog.providers.iter().flat_map(|(provider_id, provider)| {
        provider.models.keys().map(move |model_id| ModelSelection {
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
            options: Default::default(),
        })
    }).filter(|model| qaqh_profile::resolve_model_selection(catalog, model).is_ok()).collect()
}

/// `session.list` 侧栏条目:契约类型直用,status/updated_at 不经字符串化。
#[derive(Clone)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    /// meta.updated_at 原样。注意:**后端给的是秒**(list 投影把
    /// updated_at_ms/1000),只可同单位比较,不能当毫秒展示——侧栏
    /// 「N 天前」的老 bug 就是把它当毫秒算的单位错位。
    pub updated_at: u64,
    /// 创建时刻(Unix 毫秒):DB `created_at_ms` 真值优先,`ses_<ULID>`
    /// 自解码兜底;0 = 两者皆不可得。仅排序用。
    pub created_at_ms: u64,
    pub status: SessionRunStatus,
    pub workspace_id: Option<String>,
    pub cwd: Option<String>,
    pub archived: bool,
    pub handoff_from_session_id: Option<String>,
}

pub fn rows_from(entries: &[SessionListEntry]) -> Vec<SessionRow> {
    entries.iter()
        .map(|entry| SessionRow {
            id: entry.meta.session_id.clone(),
            title: entry.meta.title.clone().unwrap_or_else(|| "(无标题)".into()),
            updated_at: entry.meta.updated_at,
            created_at_ms: entry
                .meta
                .created_at_ms
                .filter(|ms| *ms > 0)
                .or_else(|| session_created_at_ms(&entry.meta.session_id))
                .unwrap_or(0),
            status: entry.status,
            workspace_id: entry.workspace_id.clone(),
            cwd: entry.meta.cwd.clone(),
            archived: entry.meta.archived,
            handoff_from_session_id: entry.meta.handoff_from_session_id.clone(),
        })
        .filter(|row| !row.id.is_empty())
        .collect()
}

/// 右栏任务条目。
pub struct TodoRow {
    pub title: String,
    pub status: String,
}

pub fn todos_from(value: &qaqh_ringing_contract::todo::TodoListSnapshot) -> Vec<TodoRow> {
    value.items.iter()
        .map(|item| TodoRow {
            title: item.title.clone(),
            status: item.status.clone(),
        })
        .collect()
}

/// 侧栏分组:workspace order 决定组序,组内 updated_at 降序;未分组兜底。
pub struct WorkspaceGroup {
    pub id: Option<String>,
    pub title: String,
    pub rows: Vec<SessionRow>,
}

pub fn groups(sessions: &[SessionRow], workspaces: &[WorkspaceEntry]) -> Vec<WorkspaceGroup> {
    let mut ordered: Vec<(Option<&WorkspaceEntry>, Vec<&SessionRow>)> = workspaces.iter().map(|entry| (Some(entry), Vec::new())).collect();
    for row in sessions {
        let workspace = row
            .workspace_id
            .as_deref()
            .and_then(|id| workspaces.iter().find(|entry| entry.id == id));
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
            // 组内新→旧:优先毫秒创建时刻(id 自解码兜底),旧数据退回
            // updated_at(秒 ×1000 归一到毫秒量纲);决胜键 session_id 字典序
            // (electron 同款绝对确定性排序)。
            let recency = |row: &SessionRow| {
                if row.created_at_ms > 0 {
                    row.created_at_ms
                } else {
                    row.updated_at.saturating_mul(1000)
                }
            };
            rows.sort_by(|a, b| recency(b).cmp(&recency(a)).then_with(|| a.id.cmp(&b.id)));
            WorkspaceGroup {
                id: entry.map(|entry| entry.id.clone()),
                title: entry.map(|entry| entry.title.clone()).unwrap_or_else(|| "未分组".into()),
                rows,
            }
        })
        .collect()
}

/// `ses_<ULID>` 会话 id → 创建时刻(Unix 毫秒)。
///
/// 生成规则在后端 qaqh-session/src/manager.rs `allocate_session` =
/// `ses_` + Crockford base32(26 字符),载荷是 UUIDv7 的 16 字节
/// (qaqh-ringing-contract/src/ulid.rs `generate_ulid`),前 48 bit 即 Unix
/// 毫秒。这里只解码前 10 个字符(50 bit,时间戳占高 48);异形 id 一律 None。
pub fn session_created_at_ms(session_id: &str) -> Option<u64> {
    let ulid = session_id.strip_prefix("ses_")?;
    let bytes = ulid.as_bytes();
    if bytes.len() != 26 {
        return None;
    }
    let mut ts: u64 = 0;
    for &ch in &bytes[..10] {
        let up = ch.to_ascii_uppercase();
        let v = match up {
            b'0'..=b'9' => u64::from(up - b'0'),
            b'A'..=b'H' => u64::from(up - b'A' + 10),
            b'J' | b'K' => u64::from(up - b'J' + 18),
            b'M' | b'N' => u64::from(up - b'M' + 20),
            b'P'..=b'T' => u64::from(up - b'P' + 22),
            b'V'..=b'Z' => u64::from(up - b'V' + 27),
            // Crockford 易混容错:I/L→1、O→0;U 不在字母表。
            b'I' | b'L' => 1,
            b'O' => 0,
            _ => return None,
        };
        ts = (ts << 5) | v;
    }
    // 首字符只带 3 个有效位(26×5=130 > 128,网格顶 2 位恒 0),前 10 字符
    // 累出的 50 bit 里时间戳正好占低 48 位,无需再移位。
    Some(ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 复刻后端 ulid.rs 的 Crockford 编码,验证解码往返一致。
    fn encode_ulid(bytes: [u8; 16]) -> String {
        const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
        let value = u128::from_be_bytes(bytes);
        let mut out = String::with_capacity(26);
        for position in (0..26).rev() {
            let index = ((value >> (position * 5)) & 0x1f) as usize;
            out.push(ALPHABET[index] as char);
        }
        out
    }

    #[test]
    fn decodes_creation_ms_from_ses_ulid() {
        // UUIDv7 前 6 字节 = Unix 毫秒(48 bit),剩余 80 bit 随机位不影响。
        let ms: u64 = 1_700_000_000_123;
        let be = ms.to_be_bytes();
        let mut bytes = [0u8; 16];
        bytes[..6].copy_from_slice(&be[2..]);
        bytes[6] = 0x70; // v7 版本位,形状保真即可。
        let id = format!("ses_{}", encode_ulid(bytes));
        assert_eq!(session_created_at_ms(&id), Some(ms));
    }

    #[test]
    fn rejects_foreign_ids() {
        assert_eq!(session_created_at_ms("ses_short"), None);
        assert_eq!(session_created_at_ms("0123abcd-uuid-shape"), None);
        assert_eq!(session_created_at_ms(""), None);
    }

    #[test]
    fn groups_sort_prefers_created_ms_then_updated_fallback() {
        let mk = |id: &str, updated_at: u64, created_at_ms: u64| SessionRow {
            id: id.into(),
            title: String::new(),
            updated_at,
            created_at_ms,
            status: SessionRunStatus::NotRunning,
            workspace_id: None,
            cwd: None,
            archived: false,
            handoff_from_session_id: None,
        };
        // ses_a 只有毫秒创建时刻,ses_b 是老数据(仅秒制 updated_at):
        // 200_000ms > 100s×1000,新会话排前。
        let old = mk("ses_b", 100, 0);
        let new = mk("ses_a", 0, 200_000);
        let grouped = groups(&[old, new], &[]);
        let ids: Vec<_> = grouped[0].rows.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["ses_a", "ses_b"]);
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
        let (method, key) = match self.service {
            AccountService::Siwc => ("siwc.sign_in.status", "flow_id"),
            AccountService::Dsaccount => ("dsaccount.sign_in.status", "attempt_id"),
        };
        let mut params = serde_json::Map::new();
        params.insert(key.into(), serde_json::Value::String(self.flow_id.clone()));
        BridgeRequest::Service {
            method: method.into(),
            params: serde_json::Value::Object(params),
        }
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
    options: &'a [qaqh_types::ModelSelection],
    current: Option<&qaqh_types::ModelSelection>,
) -> Option<&'a qaqh_types::ModelSelection> {
    let current = current.and_then(|current| options.iter().find(|model| {
        model.provider_id == current.provider_id && model.model_id == current.model_id
    }));
    next_option(options, current)
}

pub fn model_effort_levels(catalog: &qaqh_profile::ProfilesDocument, model: &qaqh_types::ModelSelection) -> Vec<String> {
    catalog.providers.get(&model.provider_id)
        .and_then(|provider| provider.models.get(&model.model_id))
        .filter(|model| model.capabilities.reasoning)
        .map(|model| model.reasoning_levels.clone()).unwrap_or_default()
}

