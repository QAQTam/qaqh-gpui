//! GPUI 实体状态(M3):字段直持 RingingV2 契约类型,零前端 DTO。
//! 多会话:visited store 常驻缓存、事件按 seed 路由;输入框统一走
//! gpui-component 的 InputState/TextareaState(InputState::new 需要
//! &mut Window,故实体构造一律在窗口上下文内完成)。

use std::collections::HashSet;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use gpui::App;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::Window;
use gpui::{ListAlignment, ListState, px};
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::BlockId;
use qaqh_ringing_contract::fact::ExecSessionId;
use qaqh_ringing_contract::fact::InteractionId;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::fact::TurnId;
use qaqh_ringing_contract::state::DriverState;
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
use gpui_component::input::{InputEvent, InputState, TextareaState};
use qaqh_client::{
    CommandTarget, RingingCommand, SessionSettingsPatch, ToolsSettingPatch,
};
use qaqh_profile::ProfilesDocument;
use qaqh_types::{ModelSelection, SessionListEntry, SettingValue};

/// 单会话 SSE 流状态(由 `V2StreamStatus` 映射,避免 store 依赖 SDK 内部枚举形状)。
pub enum StreamState {
    Connecting,
    Open,
    Reconnecting { retry_ms: u64 },
    Closed { reason: String },
}

/// ask 卡草稿:每题已选项 + 自定义输入框(焦点由 InputState 自管)。
pub struct AskDraft {
    /// 每题点选的选项文本(与 customs 互斥生效)。
    pub answers: Vec<Option<String>>,
    /// 每题自定义输入框(非空时优先于 answers)。
    pub customs: Vec<Entity<InputState>>,
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

/// ADR-0016 封箱投影:标记旧线已封箱所需的后继入口与工作简述。
/// HandoffSealed 双发(封箱线/后继线),仅封箱线本线落库;整字段替换幂等。
/// bootstrap 快照不携带本字段;封箱是不可逆历史事实,resync 后保留。
pub struct HandoffSeal {
    pub successor_session_id: String,
    pub work_summary: String,
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
    pub exec_sessions: Vec<ExecSessionView>,
    /// 执行会话输出:exec_session_id → (已收 offset 上界, 累计文本)。
    pub exec_output_tail: Vec<(ExecSessionId, u64, String)>,
    pub driver: Option<DriverState>,
    pub stream: StreamState,
    /// markdown 解析缓存(M4):section key → (源哈希, 解析结果)。
    pub md_cache: crate::markdown::ParseCache,
    /// 流式吐字速率采样:3s 滑窗,元素 = (到达时刻, 本次净增字符数)。
    /// 速率为估算(字符/4≈token);封口块由 usage/generated_ms 给权威值。
    pub tok_samples: std::collections::VecDeque<(std::time::Instant, i64)>,
    pub timeline_list: ListState,
    pub timeline_follow_setting: Option<bool>,
    pub settings: Option<SessionListEntry>,
    pub settings_pending: bool,
    pub catalog: Option<Arc<ProfilesDocument>>,
    /// 删除确认二段态(第一次点击武装,第二次执行)。
    pub confirm_delete: bool,
    /// composer 状态(M2):组件库 Textarea(auto_grow,Enter 提交,Shift+Enter 换行)。
    pub composer: Entity<TextareaState>,
    /// 重命名草稿(Some = 重命名模式)。
    pub renaming: Option<Entity<InputState>>,
    /// ask 卡草稿,按 interaction_id。
    pub ask_drafts: Vec<(InteractionId, AskDraft)>,
    /// ask 卡未答提示(上次提交被拒时显示,成功提交/跳过/解决后清除)。
    pub ask_hint: Option<String>,
    /// goal Dialog 草稿(objective, 预算);Some = 对话框打开中。
    pub goal_draft: Option<(Entity<InputState>, Entity<InputState>)>,
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
                .placeholder("输入消息,Enter 发送…")
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
            arg_stream: Vec::new(),
            output_tail: Vec::new(),
            interactions: Vec::new(),
            subagents: Vec::new(),
            goal: None,
            handoff_seal: None,
            exec_sessions: Vec::new(),
            exec_output_tail: Vec::new(),
            driver: None,
            stream: StreamState::Connecting,
            md_cache: crate::markdown::ParseCache::default(),
            tok_samples: std::collections::VecDeque::new(),
            timeline_list,
            timeline_follow_setting: None,
            settings: None,
            settings_pending: false,
            catalog: None,
            confirm_delete: false,
            composer,
            renaming: None,
            ask_drafts: Vec::new(),
            ask_hint: None,
            goal_draft: None,
            requests,
        }
    }

    /// 发送受阻原因(与 composer 提示同一来源)。
    pub fn send_block_reason(&self) -> Option<&'static str> {
        if !matches!(self.stream, StreamState::Open) {
            return Some("等待连接恢复");
        }
        if self.snapshot_pending { return Some("会话状态正在同步"); }
        if self.attachment_pending { return Some("附件正在准备"); }
        if self.outbox.as_ref().is_some_and(|o| matches!(o.state,
            qaqh_gpui_wire::delivery::DeliveryState::Sending | qaqh_gpui_wire::delivery::DeliveryState::Accepted | qaqh_gpui_wire::delivery::DeliveryState::Uncertain)) {
            return Some("上一条消息尚未确认，请先查回执");
        }
        if self.driver.as_ref().and_then(|d| d.holder.as_ref()).is_some_and(|holder| Some(holder) != self.client_session_id.as_ref()) {
            return Some("其他客户端持有控制权，可显式接管");
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

    pub fn respond_permission(&mut self, tool_call_id: String, approved: bool, trust_folder: bool) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RespondPermission {
                seed: self.session_id.clone(),
                tool_call_id,
                approved,
                trust_folder,
            });
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
    /// 取(或建)ask 卡草稿;输入框实体需窗口上下文创建。
    /// 请求被引用内容解析替换后题数可能变化,题数不匹配时重建,防越界。
    pub fn ask_draft(
        &mut self,
        interaction_id: &InteractionId,
        question_count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> &mut AskDraft {
        let position = self
            .ask_drafts
            .iter()
            .position(|(id, _)| id == interaction_id);
        let index = match position {
            Some(index) => {
                if self.ask_drafts[index].1.answers.len() != question_count {
                    self.ask_drafts[index].1 = AskDraft {
                        answers: vec![None; question_count],
                        customs: (0..question_count)
                            .map(|_| cx.new(|cx| InputState::new(window, cx).placeholder("其它…")))
                            .collect(),
                    };
                }
                index
            }
            None => {
                self.ask_drafts.push((
                    interaction_id.clone(),
                    AskDraft {
                        answers: vec![None; question_count],
                        customs: (0..question_count)
                            .map(|_| cx.new(|cx| InputState::new(window, cx).placeholder("其它…")))
                            .collect(),
                    },
                ));
                self.ask_drafts.len() - 1
            }
        };
        &mut self.ask_drafts[index].1
    }

    /// 组装应答并清草稿;未回答的题返回 None(调用方提示)。
    pub fn ask_submit(
        &mut self,
        interaction_id: &InteractionId,
        question_ids: &[String],
        cx: &Context<Self>,
    ) -> Option<Vec<qaqh_client::AskAnswer>> {
        let index = self
            .ask_drafts
            .iter()
            .position(|(id, _)| id == interaction_id)?;
        let draft = &self.ask_drafts[index].1;
        let mut answers = Vec::new();
        for (position, id) in question_ids.iter().enumerate() {
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
        Some(answers)
    }

    pub fn ask_dismiss(&mut self, interaction_id: &InteractionId) {
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

    /// 进入重命名模式,草稿取当前标题;Enter 提交由输入事件订阅驱动。
    pub fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = cx.new(|cx| InputState::new(window, cx).placeholder("新标题…"));
        cx.subscribe(&state, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                this.rename_commit(cx);
            }
        })
        .detach();
        self.renaming = Some(state);
    }

    /// Keep the edit until the authoritative metadata event confirms it.
    pub fn rename_commit(&mut self, cx: &Context<Self>) {
        let Some(draft) = self.renaming.as_ref() else {
            return;
        };
        let title = draft.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RenameSession {
                seed: self.session_id.clone(),
                title,
            });
    }

    pub fn rename_cancel(&mut self) {
        self.renaming = None;
    }

    /// handoff fork(v3.1a):从本会话的 handoff 工具事实创建交接会话。
    /// 载荷抄本会话 meta 的 cwd/model_selection(electron forkHandoffSession 同口径),
    /// title = work_summary,首消息 = 交接文档,handoff_from = 本会话(goal 工作链接缝)。
    pub fn fork_handoff(&mut self, summary: Option<String>, doc: String) {
        let meta = self.settings.as_ref().map(|settings| &settings.meta);
        let _ = self.requests.unbounded_send(BridgeRequest::CreateSession {
            cwd: meta.and_then(|meta| meta.cwd.clone()),
            model_selection: meta.and_then(|meta| meta.model_selection.clone()),
            title: summary,
            first_message: Some(doc),
            handoff_from: Some(self.session_id.clone()),
        });
    }

    /// ADR-0016 投影切换:跳后继会话(Select = attach + activate_stream + bootstrap)。
    pub fn open_handoff_successor(&mut self) {
        if let Some(seal) = &self.handoff_seal {
            let _ = self.requests.unbounded_send(BridgeRequest::Select(seal.successor_session_id.clone()));
        }
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

    /// 脱离工作区分组(落入「未分组」)。
    pub fn detach_from_workspace(&mut self) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::DetachSession {
                seed: self.session_id.clone(),
            });
    }

    /// 删除本会话(幂等;名册轮询刷新列表)。
    pub fn delete_self(&mut self) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::DeleteSession {
                seed: self.session_id.clone(),
            });
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

/// `session.list` 侧栏条目(原始 JSON 的最小投影)。
#[derive(Clone)]
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub updated_at: String,
    /// SessionRunStatus 词表字符串(running/waiting_user/cancelling/idle/not_running)。
    pub status: String,
    pub workspace_id: Option<String>,
    pub cwd: Option<String>,
    pub archived: bool,
}

pub fn rows_from(entries: &[SessionListEntry]) -> Vec<SessionRow> {
    entries.iter()
        .map(|entry| SessionRow {
            id: entry.meta.session_id.clone(),
            title: entry.meta.title.clone().unwrap_or_else(|| "(无标题)".into()),
            updated_at: entry.meta.updated_at.to_string(),
            status: match entry.status {
                qaqh_types::SessionRunStatus::NotRunning => "not_running",
                qaqh_types::SessionRunStatus::Idle => "idle",
                qaqh_types::SessionRunStatus::Running => "running",
                qaqh_types::SessionRunStatus::WaitingUser => "waiting_user",
                qaqh_types::SessionRunStatus::Cancelling => "cancelling",
            }.into(),
            workspace_id: entry.workspace_id.clone(),
            cwd: entry.meta.cwd.clone(),
            archived: entry.meta.archived,
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
            rows.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
            WorkspaceGroup {
                id: entry.map(|entry| entry.id.clone()),
                title: entry.map(|entry| entry.title.clone()).unwrap_or_else(|| "未分组".into()),
                rows,
            }
        })
        .collect()
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

