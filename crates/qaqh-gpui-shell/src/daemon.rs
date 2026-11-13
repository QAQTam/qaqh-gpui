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
use qaqh_ringing_contract::events::SessionEvent;
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::BlockId;
use qaqh_ringing_contract::fact::ExecSessionId;
use qaqh_ringing_contract::fact::InteractionId;
use qaqh_ringing_contract::fact::InteractionKind;
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

use qaqh_gpui_wire::bridge::BridgeMsg;
use qaqh_gpui_wire::bridge::BridgeRequest;
use gpui_component::input::{InputEvent, InputState, TextareaState};
use gpui_component::WindowExt;
use qaqh_client::{
    CommandTarget, RingingCommand, SessionSettingsPatch, ToolsSettingPatch,
};
use qaqh_config_api::ConfigDto;
use qaqh_profile::ProfilesDocument;
use qaqh_types::{ModelSelection, SessionListEntry, SettingValue};
use qaqh_gpui_session::store::{SessionStore, SessionRow, TodoRow, WorkspaceGroup, SettingsEditor, AccountService, AccountSignIn, StreamState, rows_from, todos_from, groups, model_options_from};
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

/// 服务面响应 → SDK 类型;解析失败以 Err 上抛,契约问题必须可见。
fn decode<T: serde::de::DeserializeOwned>(
    method: &str,
    value: serde_json::Value,
) -> Result<T, String> {
    serde_json::from_value(value).map_err(|error| format!("{method} 响应解析失败:{error}"))
}

pub struct DaemonStore {
    pub last_operation: Option<String>,
    pub connection: qaqh_gpui_wire::bridge::ConnectionState,
    pub workbench: qaqh_gpui_session::persist::Workbench,
    pub session_search: Entity<InputState>,
    pub show_archived: bool,
    pub lines: Vec<String>,
    pub sessions: Vec<SessionRow>,
    pub workspaces: Vec<WorkspaceEntry>,
    /// 侧栏分组折叠状态(组标题)。
    pub collapsed: HashSet<String>,
    /// 设置页打开状态(占用主区,侧栏常驻)。
    pub settings_open: bool,
    /// 设置页 daemon 侧数据:config.load 快照 + provider.list 结果。
    pub config: Option<ConfigDto>,
    pub providers: Option<Arc<ProfilesDocument>>,
    pub new_model: Option<ModelSelection>,
    /// 新建会话面板打开状态 + 选择(workspace 下标/effort)。
    pub new_session_open: bool,
    pub creating_session: bool,
    pub new_workspace: usize,
    /// 设置页字体输入框(Enter 应用,由订阅驱动)。
    pub font_field: Entity<InputState>,
    pub settings_editor: SettingsEditor,
    pub settings_form: Option<crate::settings_ui::Form>,
    /// 服务面账号状态(siwc=ChatGPT 套餐 / dsaccount=DeepSeek 账号)。
    pub siwc: Option<qaqh_client::service_types::SiwcStatus>,
    pub dsaccount: Option<qaqh_client::service_types::DsAccountStatus>,
    pub ds_balance: Option<qaqh_client::service_types::DsAccountBalance>,
    /// 进行中的浏览器登录流程(Some = 轮询中)。
    pub sign_in: Option<AccountSignIn>,
    /// 账号页操作结果行(错误/提示)。
    pub account_line: Option<String>,
    /// 所有访问过的会话 store(事件按 seed 路由,后台不丢)。
    pub stores: Vec<Entity<SessionStore>>,
    pub current: Option<Entity<SessionStore>>,
    /// 当前会话的 todo 快照(seed, items)。
    pub todos: Option<(String, Vec<TodoRow>)>,
    /// 当前会话的 team 快照(ADR-0014):Some((seed, response));无推送,选中/轮询刷新。
    pub team: Option<(String, qaqh_client::v2::TeamResponse)>,
    pub requests: UnboundedSender<BridgeRequest>,
}

impl DaemonStore {
    pub fn new(
        requests: UnboundedSender<BridgeRequest>,
        workbench: qaqh_gpui_session::persist::Workbench,
        load_error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session_search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索会话…"));
        // 侧栏过滤随输入实时刷新(事件类型标注同上,InputState 发多种事件)。
        cx.subscribe(&session_search, |_, _, _: &InputEvent, cx| cx.notify()).detach();
        let font_field = cx.new(|cx| InputState::new(window, cx).placeholder("系统默认…"));
        // Enter 应用字体设置。
        cx.subscribe(&font_field, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                let text = this.font_field.read(cx).value().trim().to_string();
                qaqh_gpui_kit::settings::update(cx, |content| content.typography.ui_font_family = text);
            }
        })
        .detach();
        Self {
            last_operation: None,
            connection: qaqh_gpui_wire::bridge::ConnectionState::Connecting,
            collapsed: workbench.collapsed.iter().cloned().collect(),
            workbench,
            session_search,
            show_archived: false,
            lines: vec![load_error.unwrap_or_else(|| "qaqh-gpui 启动…".into())],
            sessions: Vec::new(),
            workspaces: Vec::new(),
            settings_open: false,
            config: None,
            providers: None,
            new_model: None,
            new_session_open: false,
            creating_session: false,
            new_workspace: 0,
            font_field,
            settings_editor: SettingsEditor::default(),
            settings_form: None,
            siwc: None,
            dsaccount: None,
            ds_balance: None,
            sign_in: None,
            account_line: None,
            stores: Vec::new(),
            current: None,
            todos: None,
            team: None,
            requests,
        }
    }

    pub fn handle(&mut self, message: BridgeMsg, window: &mut Window, cx: &mut Context<Self>) {
        match message {
            BridgeMsg::FromConnection(generation, message) => {
                if generation == qaqh_gpui_wire::bridge::connection_generation() { self.handle(*message, window, cx); }
            }
            BridgeMsg::Operation(name, target, result) => {
                let label = match name.split('.').next().unwrap_or_default() {
                    "session" => "会话操作", "workspace" => "工作区操作", "provider" => "服务商设置",
                    "profile" => "预设设置", "model" => "模型设置", "config" => "全局设置",
                    "interaction" => "提问应答", "plan" => "计划评审", "tool" => "工具授权",
                    "conversation" => "对话操作", "exec_session" => "执行会话操作", "driver" => "控制权操作",
                    _ => "操作",
                };
                match result {
                    Ok(ack) => {
                        qaqh_gpui_wire::diagnostics::record(if ack.status == qaqh_client::RingingCommandAckStatus::Accepted { "command_accepted" } else { "command_rejected" },
                            match &target { qaqh_client::CommandTarget::Session { session_id } => Some(session_id.as_str()), _ => None }, Some(&ack.command_id), None);
                        if ack.status == qaqh_client::RingingCommandAckStatus::Rejected {
                            self.last_operation = Some(format!("{label} · 被拒绝 · {} {}", ack.code.unwrap_or_default(), ack.message.unwrap_or_default()));
                        } else {
                            // 成功受理走瞬时气泡;常驻状态行只承载失败/待确认。
                            let status = if ack.result.is_some() { "已完成" } else { "已受理" };
                            window.push_notification(
                                gpui_component::notification::Notification::success(
                                    format!("{label} · {status}"),
                                ),
                                cx,
                            );
                        }
                    }
                    Err(error) => {
                        self.last_operation = Some(format!("{label} · 结果待确认：{error}"));
                    }
                }
                cx.notify();
            }
            BridgeMsg::Connection(state) => {
                let id = match &state { qaqh_gpui_wire::bridge::ConnectionState::Ready { client_session_id } => Some(client_session_id.clone()), _ => None };
                self.connection = state;
                for store in &self.stores {
                    store.update(cx, |store, cx| {
                        if store.client_session_id != id {
                            store.snapshot_pending = true;
                            store.buffered_frames.clear();
                        }
                        store.client_session_id = id.clone();
                        if id.is_none() { store.stream = StreamState::Connecting; }
                        cx.notify();
                    });
                }
                cx.notify();
            }
            BridgeMsg::Delivery(seed, id, result) => {
                if let Some(store) = self.stores.iter().find(|store| store.read(cx).session_id == seed) {
                    store.update(cx, |store, cx| {
                        if let Some(outbox) = &mut store.outbox && outbox.command_id == id {
                            let was_pending = matches!(outbox.state, qaqh_gpui_wire::delivery::DeliveryState::Sending | qaqh_gpui_wire::delivery::DeliveryState::Uncertain);
                            match result {
                                qaqh_gpui_wire::bridge::DeliveryResult::Ack(ack) => outbox.acknowledge(&ack),
                                qaqh_gpui_wire::bridge::DeliveryResult::Receipt(status) => outbox.receipt(&status),
                                qaqh_gpui_wire::bridge::DeliveryResult::Unknown(detail) => {
                                    outbox.state = qaqh_gpui_wire::delivery::DeliveryState::Uncertain;
                                    outbox.detail = detail;
                                }
                            }
                            if was_pending && matches!(outbox.state, qaqh_gpui_wire::delivery::DeliveryState::Accepted | qaqh_gpui_wire::delivery::DeliveryState::Succeeded)
                                && store.composer.read(cx).value().trim() == outbox.message.text {
                                store.composer.update(cx, |composer, cx| composer.set_value("", window, cx));
                                store.images.clear(); store.attachments.clear();
                            }
                            if outbox.state == qaqh_gpui_wire::delivery::DeliveryState::Succeeded {
                                outbox.message.images.clear(); outbox.message.attachments = None; outbox.message.text.clear();
                            }
                        }
                        if let Err(error) = store.persist_draft(cx) { store.operation_error = Some(error); }
                        cx.notify();
                    });
                }
            }
            BridgeMsg::Attachment(seed, result) => {
                if let Some(store) = self.stores.iter().find(|store| store.read(cx).session_id == seed) {
                    store.update(cx, |store, cx| {
                        store.attachment_pending = false;
                        match result {
                            Ok(qaqh_gpui_wire::bridge::PreparedAttachment::Image(image)) => store.images.push(image),
                            Ok(qaqh_gpui_wire::bridge::PreparedAttachment::Content(reference)) => store.attachments.push(reference),
                            Err(error) => store.operation_error = Some(error),
                        }
                        cx.notify();
                    });
                }
            }
            BridgeMsg::ResolvedContent(seed, reference, result) => {
                if let Some(store) = self.stores.iter().find(|store| store.read(cx).session_id == seed) {
                    store.update(cx, |store, cx| {
                        match result {
                            Ok(text) => qaqh_gpui_session::content::replace(store, &reference, text),
                            Err(error) => store.operation_error = Some(error),
                        }
                        cx.notify();
                    });
                }
            }
            BridgeMsg::Line(text) => {
                self.lines.push(text);
                if self.lines.len() > 200 { self.lines.drain(..100); }
                cx.notify();
            }
            BridgeMsg::Sessions(value) => {
                crate::desktop::roster_notifications(&self.sessions, &value, cx);
                self.sessions = rows_from(&value);
                for store in &self.stores {
                    let id = &store.read(cx).session_id;
                    if let Some(row) = self.sessions.iter().find(|row| &row.id == id) {
                        let cwd = row.cwd.as_deref().map(Into::into);
                        store.update(cx, |store, _| store.code_panel.cwd = cwd);
                    }
                }
                if self.current.is_none()
                    && let Some(id) = self.workbench.recent_session.as_ref()
                        .and_then(|id| self.sessions.iter().find(|row| &row.id == id))
                        .or_else(|| self.sessions.iter().find(|row| !row.archived)).map(|row| row.id.clone())
                {
                    self.select(id, window, cx);
                    return;
                }
                cx.notify();
            }
            BridgeMsg::Workspaces(entries) => {
                self.workspaces = entries;
                cx.notify();
            }
            BridgeMsg::Todos(seed, value) => {
                let current_seed = self
                    .current
                    .as_ref()
                    .map(|current| current.read(cx).session_id.clone());
                if current_seed.as_deref() == Some(seed.as_str()) {
                    self.todos = Some((seed, todos_from(&value)));
                    cx.notify();
                }
            }
            BridgeMsg::Team(seed, value) => {
                // 只对当前选中会话生效;花名册是粗投影,精确状态看 session.list。
                let current_seed = self
                    .current
                    .as_ref()
                    .map(|current| current.read(cx).session_id.clone());
                if current_seed.as_deref() == Some(seed.as_str()) {
                    self.team = Some((seed, *value));
                    cx.notify();
                }
            }
            BridgeMsg::Event(seed, frame) => {
                // ask 请求在 frame 被消费前先摘出 id(apply_frame 会 move frame)。
                let ask_arrival = match &frame.event {
                    SessionEvent::InteractionRequested { interaction }
                        if interaction.kind == InteractionKind::Ask =>
                    {
                        Some(interaction.interaction_id.clone())
                    }
                    _ => None,
                };
                let notification = match &frame.event {
                    SessionEvent::InteractionRequested { .. } => Some("会话等待你的响应"),
                    SessionEvent::TurnEnded { outcome, .. } => Some(match outcome {
                        qaqh_ringing_contract::state::TurnOutcome::Finished { error: Some(_), .. } => "本轮工作失败，请查看会话",
                        _ => "本轮工作已结束",
                    }),
                    _ => None,
                };
                let review_turn = match &frame.event {
                    SessionEvent::TurnEnded { turn_id, .. }
                        if self.current.as_ref().is_some_and(|store| store.read(cx).session_id == seed) =>
                    {
                        Some(turn_id.clone())
                    }
                    _ => None,
                };
                // 工具完成/回合结束刷新 todo(与 electron 同触发时机)。
                let refresh_todos = matches!(
                    frame.event,
                    SessionEvent::ToolFinished { .. } | SessionEvent::TurnEnded { .. }
                );
                // 任务板/留言板工具开工即刷新 team 快照(无 delta 推送,施工单 §一·3 主触发)。
                let refresh_team = matches!(&frame.event, SessionEvent::ToolDeclared { tool_name, .. }
                    if tool_name.starts_with("task_") || tool_name.starts_with("board_"));
                let store = self
                    .stores
                    .iter()
                    .find(|store| store.read(cx).session_id == seed)
                    .cloned();
                let Some(store) = store else {
                    return;
                };
                store.update(cx, |store, cx| {
                    if store.snapshot_pending {
                        store.buffered_frames.push(frame);
                        if store.buffered_frames.len() >= 256 {
                            store.buffered_frames.clear();
                            let _ = store.requests.unbounded_send(BridgeRequest::Refill(store.session_id.clone()));
                        }
                        return;
                    }
                    let previous_seq = store.seq;
                    qaqh_gpui_session::reducer::apply_frame(store, frame, cx);
                    if store.seq > previous_seq && let Some(title) = notification { crate::desktop::notify(cx, &store.session_id, title); }
                    if store.seq > previous_seq && let Some(turn) = review_turn {
                        qaqh_gpui_session::code_panel::auto_review(store, turn);
                    }
                    cx.notify();
                });
                // ask 请求到达即弹模态应答卡(仅当前会话);关闭后可经 footer 按钮重开。
                // 需在 seed 被 QueryTodo 移动之前判定。
                if let Some(iid) = ask_arrival
                    && let Some(entity) = self
                        .current
                        .as_ref()
                        .filter(|store| store.read(cx).session_id == seed)
                {
                    qaqh_gpui_session::timeline::open_ask_dialog(entity, iid, window, cx);
                }
                if refresh_todos {
                    let _ = self
                        .requests
                        .unbounded_send(BridgeRequest::QueryTodo { seed: seed.clone() });
                }
                if refresh_team {
                    let _ = self
                        .requests
                        .unbounded_send(BridgeRequest::QueryTeam { seed });
                }
            }
            BridgeMsg::Reset(seed, _reset) => {
                let store = self
                    .stores
                    .iter()
                    .find(|store| store.read(cx).session_id == seed)
                    .cloned();
                if let Some(store) = store {
                    store.update(cx, |store, cx| {
                        store.snapshot_pending = true;
                        store.buffered_frames.clear();
                        qaqh_gpui_session::reducer::clear_transient(store);
                        cx.notify();
                    });
                }
                let _ = self.requests.unbounded_send(BridgeRequest::Refill(seed));
            }
            BridgeMsg::Snapshot(state) => {
                let seed = state
                    .session_id
                    .as_ref()
                    .map(|id| id.0.clone())
                    .unwrap_or_default();
                let store = self
                    .stores
                    .iter()
                    .find(|store| store.read(cx).session_id == seed)
                    .cloned();
                let Some(store) = store else {
                    return;
                };
                store.update(cx, |store, cx| {
                    qaqh_gpui_session::reducer::apply_snapshot(store, *state);
                    qaqh_gpui_wire::diagnostics::record("snapshot", Some(&store.session_id), None, Some(store.seq));
                    store.snapshot_pending = false;
                    for frame in std::mem::take(&mut store.buffered_frames) { qaqh_gpui_session::reducer::apply_frame(store, frame, cx); }
                    cx.notify();
                });
                let _ = self.requests.unbounded_send(BridgeRequest::QueryTodo { seed });
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
                let store = self
                    .stores
                    .iter()
                    .find(|store| store.read(cx).session_id == seed)
                    .cloned();
                if let Some(store) = store {
                    store.update(cx, |store, cx| {
                        store.stream = stream;
                        cx.notify();
                    });
                }
            }
            BridgeMsg::Created(seed, cwd) => {
                self.lines.push(format!("会话已创建:{seed}"));
                self.new_session_open = false;
                self.creating_session = false;
                self.select(seed, window, cx);
                if let Some(store) = &self.current {
                    store.update(cx, |store, cx| {
                        store.code_panel.cwd = cwd.map(Into::into);
                        cx.notify();
                    });
                }
            }
            BridgeMsg::CreateFailed => {
                self.creating_session = false;
                cx.notify();
            }
            BridgeMsg::ReviewContent(seed, turn, request, results) => {
                if let Some(store) = self.stores.iter().find(|store| store.read(cx).session_id == seed) {
                    store.update(cx, |store, cx| {
                        qaqh_gpui_session::code_panel::finish_review(store, turn, request, results);
                        cx.notify();
                    });
                }
            }
            BridgeMsg::Config(value) => {
                self.config = Some(value);
                cx.notify();
            }
            BridgeMsg::Providers(value) => {
                let catalog = Arc::new(value);
                for store in &self.stores {
                    store.update(cx, |store, cx| {
                        store.catalog = Some(catalog.clone());
                        cx.notify();
                    });
                }
                if !self.new_model.as_ref().is_some_and(|model| qaqh_profile::resolve_model_selection(&catalog, model).is_ok()) {
                    self.new_model = catalog.default_model_selection.as_ref()
                        .filter(|model| qaqh_profile::resolve_model_selection(&catalog, model).is_ok())
                        .cloned()
                        .or_else(|| model_options_from(&catalog).into_iter().next());
                }
                self.providers = Some(catalog);
                cx.notify();
            }
            BridgeMsg::SessionSettings { seed, settings, write_completed } => {
                if let Some(store) = self.stores.iter().find(|store| store.read(cx).session_id == seed) {
                    store.update(cx, |store, cx| {
                        store.settings = settings;
                        if write_completed {
                            store.settings_pending = false;
                        }
                        cx.notify();
                    });
                }
            }
            BridgeMsg::Watchdog(seed) => {
                // 取消看门狗到点:仍在运行则本地收口暂态并重灌快照。
                let store = self
                    .stores
                    .iter()
                    .find(|store| store.read(cx).session_id == seed)
                    .cloned();
                if let Some(store) = store
                    && store.read(cx).activity == Some(ActivityState::Running)
                {
                    store.update(cx, |store, cx| {
                        qaqh_gpui_session::reducer::clear_transient(store);
                        cx.notify();
                    });
                    let _ = self.requests.unbounded_send(BridgeRequest::Refill(seed));
                    self.lines.push("取消超时:已本地收口并重灌快照".into());
                }
            }
            BridgeMsg::ServiceResult { method, result } => {
                self.handle_service_result(&method, result, cx);
                cx.notify();
            }
        }
    }

    /// 服务面请求(空参)。
    fn service(&self, method: &str) {
        let _ = self.requests.unbounded_send(BridgeRequest::Service {
            method: method.into(),
            params: serde_json::Value::Object(Default::default()),
        });
    }

    /// 打开账号页:拉两路账号状态。
    pub fn refresh_accounts(&self) {
        self.service("siwc.status");
        self.service("dsaccount.status");
    }

    /// 浏览器登录入口:起流程并打开授权页;轮询由 2s 定时器驱动。
    pub fn sign_in_start(&mut self, service: AccountService) {
        let method = match service {
            AccountService::Siwc => "siwc.sign_in.start",
            AccountService::Dsaccount => "dsaccount.sign_in.start",
        };
        self.service(method);
    }

    pub fn sign_in_cancel(&mut self) {
        let Some(flow) = self.sign_in.take() else { return };
        let (method, key) = match flow.service {
            AccountService::Siwc => ("siwc.sign_in.cancel", "flow_id"),
            AccountService::Dsaccount => ("dsaccount.sign_in.cancel", "attempt_id"),
        };
        let mut params = serde_json::Map::new();
        params.insert(key.into(), serde_json::Value::String(flow.flow_id));
        let _ = self.requests.unbounded_send(BridgeRequest::Service {
            method: method.into(),
            params: serde_json::Value::Object(params),
        });
    }

    pub fn sign_out(&mut self, service: AccountService) {
        let method = match service {
            AccountService::Siwc => "siwc.sign_out",
            AccountService::Dsaccount => "dsaccount.sign_out",
        };
        self.service(method);
    }

    /// 2s 定时器:登录流程轮询 + 过期收口。
    pub fn poll_sign_in(&mut self) {
        let Some(flow) = &self.sign_in else { return };
        if now_ms() > flow.expires_at_ms {
            self.sign_in = None;
            self.account_line = Some("登录流程已过期，请重试".into());
            return;
        }
        let request = flow.poll_request();
        let _ = self.requests.unbounded_send(request);
    }

    /// 服务面结果路由:按 method 反序列化进 SDK service_types,桥侧零转换;
    /// 解析失败显式报错,契约问题必须可见。
    fn handle_service_result(
        &mut self,
        method: &str,
        result: Result<serde_json::Value, String>,
        cx: &mut Context<Self>,
    ) {
        use qaqh_client::service_types as st;
        let Ok(value) = result else {
            self.account_line = Some(result.unwrap_err());
            return;
        };
        match method {
            "siwc.status" => match decode(method, value) {
                Ok(status) => self.siwc = Some(status),
                Err(error) => self.account_line = Some(error),
            },
            "dsaccount.status" => {
                match decode(method, value) {
                    Ok(status) => self.dsaccount = Some(status),
                    Err(error) => self.account_line = Some(error),
                }
                if self.dsaccount.as_ref().is_some_and(|status| status.signed_in)
                    && self.ds_balance.is_none()
                {
                    self.service("dsaccount.balance");
                }
            }
            "dsaccount.balance" => match decode(method, value) {
                Ok(balance) => self.ds_balance = Some(balance),
                Err(error) => self.account_line = Some(error),
            },
            "siwc.sign_in.start" => match decode::<st::SiwcSignInStart>(method, value) {
                Ok(start) => {
                    self.sign_in = Some(AccountSignIn {
                        service: AccountService::Siwc,
                        flow_id: start.flow_id,
                        expires_at_ms: start.expires_at_ms,
                    });
                    cx.open_url(&start.authorize_url);
                    self.account_line = Some("已在系统浏览器打开授权页，请在浏览器完成登录".into());
                }
                Err(error) => self.account_line = Some(error),
            },
            "siwc.sign_in.status" => match decode::<st::SiwcSignInStatus>(method, value) {
                Ok(status) => match status.state {
                    st::SiwcFlowState::Completed => {
                        self.sign_in = None;
                        self.service("siwc.status");
                        self.account_line = Some("ChatGPT 套餐已登录".into());
                    }
                    st::SiwcFlowState::Failed => {
                        self.sign_in = None;
                        self.account_line = Some(match status.error {
                            Some(error) => format!("登录失败:{}", error.message),
                            None => "登录失败".into(),
                        });
                    }
                    st::SiwcFlowState::Pending => {}
                },
                Err(error) => self.account_line = Some(error),
            },
            "siwc.sign_in.cancel" => {
                self.sign_in = None;
                self.service("siwc.status");
            }
            "siwc.sign_out" => match decode::<st::SiwcSignOutResult>(method, value) {
                Ok(out) => {
                    self.account_line = Some(if out.revoked {
                        "已登出并撤销远端授权".into()
                    } else {
                        "已登出；远端撤销未成功，可稍后在 ChatGPT 设置中断开".into()
                    });
                    self.service("siwc.status");
                }
                Err(error) => self.account_line = Some(error),
            },
            "dsaccount.sign_in.start" => {
                match decode::<st::DsAccountSignInStart>(method, value) {
                    Ok(start) => {
                        self.sign_in = Some(AccountSignIn {
                            service: AccountService::Dsaccount,
                            flow_id: start.attempt_id,
                            expires_at_ms: now_ms() + start.expires_in.saturating_mul(1000),
                        });
                        cx.open_url(&start.authorize_url);
                        self.account_line =
                            Some("已在系统浏览器打开授权页，请在浏览器完成登录".into());
                    }
                    Err(error) => self.account_line = Some(error),
                }
            }
            "dsaccount.sign_in.status" => {
                match decode::<st::DsAccountSignInStatus>(method, value) {
                    Ok(status) => match status.state {
                        st::DsFlowState::SignedIn => {
                            self.sign_in = None;
                            self.ds_balance = None;
                            self.service("dsaccount.status");
                            self.account_line = Some("DeepSeek 账号已登录".into());
                        }
                        st::DsFlowState::Failed | st::DsFlowState::Cancelled => {
                            self.sign_in = None;
                            self.account_line = Some(match status.error {
                                Some(error) => format!("登录未完成:{error}"),
                                None => "登录未完成".into(),
                            });
                        }
                        _ => {}
                    },
                    Err(error) => self.account_line = Some(error),
                }
            }
            "dsaccount.sign_in.cancel" => {
                self.sign_in = None;
                self.service("dsaccount.status");
            }
            "dsaccount.sign_out" => {
                self.dsaccount = None;
                self.ds_balance = None;
                self.service("dsaccount.status");
                self.account_line = Some("已登出 DeepSeek 账号".into());
            }
            _ => {}
        }
    }

    /// 新建会话:cwd 取所选工作区路径;创建成功经 BridgeMsg::Created 自动选中。
    /// 模型未选 = None(继承全局默认,eb3f31c 的 model_selection Inherit 口径)。
    pub fn create_session(&mut self) {
        if self.creating_session {
            return;
        }
        let cwd = self
            .workspaces
            .get(self.new_workspace)
            .map(|workspace| workspace.path.clone());
        self.creating_session = self.requests.unbounded_send(BridgeRequest::CreateSession {
            cwd,
            model_selection: self.new_model.clone(),
            title: None,
            first_message: None,
            handoff_from: None,
        }).is_ok();
    }

    pub fn select(&mut self, seed: String, window: &mut Window, cx: &mut Context<Self>) {
        self.persist(cx);
        self.workbench.recent_session = Some(seed.clone());
        // 已访问过的会话直接复用 store(状态/流保持);新会话才 attach。
        let existing = self
            .stores
            .iter()
            .find(|store| store.read(cx).session_id == seed)
            .cloned();
        match existing {
            Some(store) => {
                // LRU:复用的会话移到末尾,避免被新会话挤出缓存。
                self.stores.retain(|cached| cached != &store);
                self.stores.push(store.clone());
                store.update(cx, |store, _| { store.snapshot_pending = true; store.buffered_frames.clear(); });
                let _ = self.requests.unbounded_send(BridgeRequest::Select(seed));
                self.current = Some(store);
                cx.notify();
            }
            None => {
                let requests = self.requests.clone();
                let draft = self.workbench.drafts.get(&seed).cloned().unwrap_or_default();
                let store = cx.new(|cx| SessionStore::new(seed.clone(), requests, draft, window, cx));
                store.update(cx, |store, _| {
                    store.catalog = self.providers.clone();
                    store.client_session_id = match &self.connection {
                        qaqh_gpui_wire::bridge::ConnectionState::Ready { client_session_id } => Some(client_session_id.clone()), _ => None,
                    };
                });
                if let Some(cwd) = self.sessions.iter().find(|row| row.id == seed)
                    .and_then(|row| row.cwd.as_deref())
                {
                    store.update(cx, |store, _| store.code_panel.cwd = Some(cwd.into()));
                }
                self.stores.push(store.clone());
                self.current = Some(store);
                if self.stores.len() > 4 {
                    self.stores.remove(0);
                }
                let _ = self.requests.unbounded_send(BridgeRequest::Select(seed));
                cx.notify();
            }
        }
    }

    pub fn persist(&mut self, cx: &gpui::App) {
        for store in &self.stores {
            let store = store.read(cx);
            self.workbench.drafts.insert(store.session_id.clone(), store.draft(cx));
        }
        self.workbench.collapsed = self.collapsed.iter().cloned().collect();
        if let Err(error) = qaqh_gpui_session::persist::save(&self.workbench) {
            if self.lines.last() != Some(&error) { self.lines.push(error); }
        }
    }
}