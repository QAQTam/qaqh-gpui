//! GPUI 实体状态(M2):字段直持 RingingV2 契约类型,零前端 DTO。
//! composer 为轻量 key_down 输入(gpui 0.2.2 无内置 Input 组件)。

use futures::channel::mpsc::UnboundedSender;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::FocusHandle;
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::BlockId;
use qaqh_ringing_contract::fact::ExecSessionId;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::fact::TurnId;
use qaqh_ringing_contract::state::DriverState;
use qaqh_ringing_contract::state::ExecSessionView;
use qaqh_ringing_contract::state::InteractionView;
use qaqh_ringing_contract::state::SubagentView;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_ringing_contract::state::TurnView;

use crate::bridge::BridgeMsg;
use crate::bridge::BridgeRequest;

/// 单会话 SSE 流状态(由 `V2StreamStatus` 映射,避免 store 依赖 SDK 内部枚举形状)。
pub enum StreamState {
    Connecting,
    Open,
    Reconnecting { retry_ms: u64 },
    Closed { reason: String },
}

pub struct SessionStore {
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
    /// 未 declare 的参数生成流:call_id → (tool_name, 累计替换文本)。
    pub arg_stream: Vec<(ToolCallId, String, String)>,
    /// 运行中工具的输出尾窗:call_id → 累计文本。
    pub output_tail: Vec<(ToolCallId, String)>,
    pub interactions: Vec<InteractionView>,
    pub subagents: Vec<SubagentView>,
    pub exec_sessions: Vec<ExecSessionView>,
    /// 执行会话输出:exec_session_id → (已收 offset 上界, 累计文本)。
    pub exec_output_tail: Vec<(ExecSessionId, u64, String)>,
    pub driver: Option<DriverState>,
    pub stream: StreamState,
    /// composer 状态(M2)。
    pub composer: String,
    pub composer_focus: FocusHandle,
    requests: UnboundedSender<BridgeRequest>,
}

impl SessionStore {
    pub fn new(
        session_id: String,
        requests: UnboundedSender<BridgeRequest>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            session_id,
            title: None,
            activity: None,
            current_turn_id: None,
            seq: 0,
            turns: Vec::new(),
            entries: Vec::new(),
            streaming: Vec::new(),
            tools: Vec::new(),
            arg_stream: Vec::new(),
            output_tail: Vec::new(),
            interactions: Vec::new(),
            subagents: Vec::new(),
            exec_sessions: Vec::new(),
            exec_output_tail: Vec::new(),
            driver: None,
            stream: StreamState::Connecting,
            composer: String::new(),
            composer_focus: cx.focus_handle(),
            requests,
        }
    }

    pub fn composer_push(&mut self, text: &str) {
        self.composer.push_str(text);
    }

    pub fn composer_backspace(&mut self) {
        self.composer.pop();
    }

    /// 发送 composer 内容;空文本不发。
    pub fn composer_send(&mut self) {
        let text = self.composer.trim().to_string();
        if text.is_empty() {
            return;
        }
        self.composer.clear();
        let _ = self.requests.unbounded_send(BridgeRequest::SendMessage {
            seed: self.session_id.clone(),
            text,
        });
    }

    pub fn respond_permission(&mut self, tool_call_id: String, approved: bool) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RespondPermission {
                seed: self.session_id.clone(),
                tool_call_id,
                approved,
            });
    }

    pub fn respond_plan(&mut self, interaction_id: String, approved: bool) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RespondPlan {
                seed: self.session_id.clone(),
                interaction_id,
                approved,
            });
    }

    pub fn cancel_turn(&mut self) {
        let _ = self.requests.unbounded_send(BridgeRequest::Cancel {
            seed: self.session_id.clone(),
            turn_id: self.current_turn_id.as_ref().map(|id| id.0.clone()),
        });
    }
}

/// `session.list` 侧栏条目(sanitize 后 JSON 的最小投影)。
pub struct SessionRow {
    pub id: String,
    pub title: String,
    pub updated_at: String,
}

fn rows_from(value: &serde_json::Value) -> Vec<SessionRow> {
    value
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .map(|entry| SessionRow {
            id: entry["session_id"].as_str().unwrap_or("").to_string(),
            title: entry["title"]
                .as_str()
                .unwrap_or("(无标题)")
                .to_string(),
            updated_at: entry["updated_at"].as_str().unwrap_or("").to_string(),
        })
        .filter(|row| !row.id.is_empty())
        .collect()
}

pub struct DaemonStore {
    pub lines: Vec<String>,
    pub sessions: Vec<SessionRow>,
    pub current: Option<Entity<SessionStore>>,
    pub requests: UnboundedSender<BridgeRequest>,
}

impl DaemonStore {
    pub fn new(requests: UnboundedSender<BridgeRequest>) -> Self {
        Self {
            lines: vec!["qaqh-gpui 启动…".into()],
            sessions: Vec::new(),
            current: None,
            requests,
        }
    }

    pub fn handle(&mut self, message: BridgeMsg, cx: &mut Context<Self>) {
        match message {
            BridgeMsg::Line(text) => {
                self.lines.push(text);
                cx.notify();
            }
            BridgeMsg::Sessions(value) => {
                self.sessions = rows_from(&value);
                let first = self.sessions.first().map(|row| row.id.clone());
                if self.current.is_none()
                    && let Some(id) = first
                {
                    self.select(id, cx);
                    return;
                }
                cx.notify();
            }
            BridgeMsg::Event(seed, frame) => {
                let Some(current) = self.current.clone() else {
                    return;
                };
                if current.read(cx).session_id != seed {
                    return;
                }
                current.update(cx, |store, cx| {
                    crate::reducer::apply_frame(store, frame);
                    cx.notify();
                });
            }
            BridgeMsg::Reset(seed, _reset) => {
                let Some(current) = self.current.clone() else {
                    return;
                };
                if current.read(cx).session_id != seed {
                    return;
                }
                current.update(cx, |store, cx| {
                    crate::reducer::clear_transient(store);
                    cx.notify();
                });
                let _ = self.requests.unbounded_send(BridgeRequest::Refill(seed));
            }
            BridgeMsg::Snapshot(state) => {
                let seed = state
                    .session_id
                    .as_ref()
                    .map(|id| id.0.clone())
                    .unwrap_or_default();
                let Some(current) = self.current.clone() else {
                    return;
                };
                if current.read(cx).session_id != seed {
                    return;
                }
                current.update(cx, |store, cx| {
                    crate::reducer::apply_snapshot(store, *state);
                    cx.notify();
                });
            }
            BridgeMsg::Stream(seed, status) => {
                let stream = match status {
                    qaqh_client::v2_stream::V2StreamStatus::Connecting => StreamState::Connecting,
                    qaqh_client::v2_stream::V2StreamStatus::Open { .. } => StreamState::Open,
                    qaqh_client::v2_stream::V2StreamStatus::Reconnecting { retry_ms, .. } => {
                        StreamState::Reconnecting { retry_ms }
                    }
                    qaqh_client::v2_stream::V2StreamStatus::Closed { reason, .. } => {
                        StreamState::Closed { reason }
                    }
                };
                let Some(current) = self.current.clone() else {
                    return;
                };
                if current.read(cx).session_id != seed {
                    return;
                }
                current.update(cx, |store, cx| {
                    store.stream = stream;
                    cx.notify();
                });
            }
            BridgeMsg::Liveness => {}
        }
    }

    pub fn select(&mut self, seed: String, cx: &mut Context<Self>) {
        let requests = self.requests.clone();
        self.current = Some(cx.new(|cx| SessionStore::new(seed.clone(), requests, cx)));
        let _ = self.requests.unbounded_send(BridgeRequest::Select(seed));
        cx.notify();
    }
}
