//! GPUI 实体状态(M3):字段直持 RingingV2 契约类型,零前端 DTO。
//! 多会话:visited store 常驻缓存、事件按 seed 路由;输入框统一走
//! text_input::TextField(gpui 0.2.2 无内置 Input 组件)。

use std::collections::HashSet;
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use gpui::AppContext as _;
use gpui::Context;
use gpui::Entity;
use gpui::FocusHandle;
use gpui::{ListAlignment, ListState, px};
use qaqh_ringing_contract::events::SessionEvent;
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::BlockId;
use qaqh_ringing_contract::fact::ExecSessionId;
use qaqh_ringing_contract::fact::InteractionId;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::fact::TurnId;
use qaqh_ringing_contract::state::DriverState;
use qaqh_ringing_contract::state::ExecSessionView;
use qaqh_ringing_contract::state::InteractionView;
use qaqh_ringing_contract::state::SubagentView;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_ringing_contract::state::TurnView;
use qaqh_ringing_contract::workspace::WorkspaceEntry;
use serde_json::Value;

use crate::bridge::BridgeMsg;
use crate::bridge::BridgeRequest;
use crate::text_input::TextField;
use qaqh_client::{SessionSettingsPatch, ToolsSettingPatch};
use qaqh_config_api::ConfigDto;
use qaqh_profile::ProfilesDocument;
use qaqh_types::{ModelSelection, SessionListEntry, SettingValue};

/// 单会话 SSE 流状态(由 `V2StreamStatus` 映射,避免 store 依赖 SDK 内部枚举形状)。
pub enum StreamState {
    Connecting,
    Open,
    Reconnecting { retry_ms: u64 },
    Closed { reason: String },
}

/// ask 卡草稿:每题已选项 + 自定义输入框 + 输入焦点。
pub struct AskDraft {
    /// 每题点选的选项文本(与 customs 互斥生效)。
    pub answers: Vec<Option<String>>,
    /// 每题自定义输入框(非空时优先于 answers)。
    pub customs: Vec<TextField>,
    /// 每题自定义输入焦点。
    pub focuses: Vec<FocusHandle>,
}

pub struct SessionStore {
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
    /// markdown 解析缓存(M4):section key → (源哈希, 解析结果)。
    pub md_cache: crate::markdown::ParseCache,
    pub timeline_list: ListState,
    pub timeline_follow_setting: Option<bool>,
    pub settings: Option<SessionListEntry>,
    pub settings_pending: bool,
    pub catalog: Option<Arc<ProfilesDocument>>,
    /// 删除确认二段态(第一次点击武装,第二次执行)。
    pub confirm_delete: bool,
    /// composer 状态(M2):TextField 缓冲 + IME。
    pub composer: TextField,
    pub composer_focus: FocusHandle,
    /// 重命名草稿(Some = 重命名模式)。
    pub renaming: Option<TextField>,
    pub rename_focus: FocusHandle,
    /// ask 卡草稿,按 interaction_id。
    pub ask_drafts: Vec<(InteractionId, AskDraft)>,
    requests: UnboundedSender<BridgeRequest>,
}

impl SessionStore {
    pub fn new(
        session_id: String,
        requests: UnboundedSender<BridgeRequest>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            code_panel: Default::default(),
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
            exec_sessions: Vec::new(),
            exec_output_tail: Vec::new(),
            driver: None,
            stream: StreamState::Connecting,
            md_cache: crate::markdown::ParseCache::default(),
            timeline_list: ListState::new(1, ListAlignment::Top, px(200.)),
            timeline_follow_setting: None,
            settings: None,
            settings_pending: false,
            catalog: None,
            confirm_delete: false,
            composer: TextField::default(),
            composer_focus: cx.focus_handle(),
            renaming: None,
            rename_focus: cx.focus_handle(),
            ask_drafts: Vec::new(),
            requests,
        }
    }

    /// 发送受阻原因(与 composer 提示同一来源)。
    pub fn send_block_reason(&self) -> Option<&'static str> {
        if let StreamState::Closed { .. } = self.stream {
            return Some("连接已断开");
        }
        if !self.interactions.is_empty() {
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
    pub fn composer_send(&mut self) -> Option<&'static str> {
        if let Some(reason) = self.send_block_reason() {
            return Some(reason);
        }
        let text = self.composer.text.trim().to_string();
        if text.is_empty() {
            return None;
        }
        self.composer.clear();
        let _ = self.requests.unbounded_send(BridgeRequest::SendMessage {
            seed: self.session_id.clone(),
            text,
        });
        None
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

    pub fn respond_plan(&mut self, interaction_id: String, approved: bool) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RespondPlan {
                seed: self.session_id.clone(),
                interaction_id,
                approved,
            });
    }

    /// 取(或建)ask 卡草稿;focus 句柄需 cx 创建。
    pub fn ask_draft(
        &mut self,
        interaction_id: &InteractionId,
        question_count: usize,
        cx: &mut Context<Self>,
    ) -> &mut AskDraft {
        let position = self
            .ask_drafts
            .iter()
            .position(|(id, _)| id == interaction_id);
        let index = match position {
            Some(index) => index,
            None => {
                self.ask_drafts.push((
                    interaction_id.clone(),
                    AskDraft {
                        answers: vec![None; question_count],
                        customs: (0..question_count)
                            .map(|_| TextField::default())
                            .collect(),
                        focuses: (0..question_count)
                            .map(|_| cx.focus_handle())
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
                .map(|field| field.text.trim().to_string())
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
        self.ask_drafts.remove(index);
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
        self.ask_drafts
            .retain(|(id, _)| id != interaction_id);
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

    /// 进入重命名模式,草稿取当前标题。
    pub fn begin_rename(&mut self) {
        let current = self.title.clone().unwrap_or_default();
        self.renaming = Some(TextField::with_text(&current));
    }

    /// 提交重命名:乐观改本地标题,命令失败由 SessionMetaChanged 对账。
    pub fn rename_commit(&mut self) {
        let Some(draft) = self.renaming.take() else {
            return;
        };
        let title = draft.text.trim().to_string();
        if title.is_empty() {
            return;
        }
        self.title = Some(title.clone());
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
}

fn rows_from(value: &Value) -> Vec<SessionRow> {
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
            status: entry["status"].as_str().unwrap_or("not_running").to_string(),
            workspace_id: entry["workspace_id"].as_str().map(str::to_string),
            cwd: entry["cwd"].as_str().map(str::to_string),
        })
        .filter(|row| !row.id.is_empty())
        .collect()
}

/// 右栏任务条目。
pub struct TodoRow {
    pub title: String,
    pub status: String,
}

fn todos_from(value: &Value) -> Vec<TodoRow> {
    value
        .get("items")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .map(|item| TodoRow {
            title: item["title"].as_str().unwrap_or("").to_string(),
            status: item["status"].as_str().unwrap_or("pending").to_string(),
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
    let mut ordered: Vec<(Option<&WorkspaceEntry>, Vec<&SessionRow>)> = Vec::new();
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
}

pub struct DaemonStore {
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
    /// 设置页字体输入框(IME 经 DaemonStore 的 EntityInputHandler 路由)。
    pub font_field: crate::text_input::TextField,
    pub font_focus: FocusHandle,
    pub settings_editor: SettingsEditor,
    pub settings_form: Option<crate::settings_ui::Form>,
    /// 所有访问过的会话 store(事件按 seed 路由,后台不丢)。
    pub stores: Vec<Entity<SessionStore>>,
    pub current: Option<Entity<SessionStore>>,
    /// 当前会话的 todo 快照(seed, items)。
    pub todos: Option<(String, Vec<TodoRow>)>,
    pub requests: UnboundedSender<BridgeRequest>,
}

impl DaemonStore {
    pub fn new(requests: UnboundedSender<BridgeRequest>, cx: &mut Context<Self>) -> Self {
        Self {
            lines: vec!["qaqh-gpui 启动…".into()],
            sessions: Vec::new(),
            workspaces: Vec::new(),
            collapsed: HashSet::new(),
            settings_open: false,
            config: None,
            providers: None,
            new_model: None,
            new_session_open: false,
            creating_session: false,
            new_workspace: 0,
            font_field: crate::text_input::TextField::default(),
            font_focus: cx.focus_handle(),
            settings_editor: SettingsEditor::default(),
            settings_form: None,
            stores: Vec::new(),
            current: None,
            todos: None,
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
                for store in &self.stores {
                    let id = &store.read(cx).session_id;
                    if let Some(row) = self.sessions.iter().find(|row| &row.id == id) {
                        let cwd = row.cwd.as_deref().map(Into::into);
                        store.update(cx, |store, _| store.code_panel.cwd = cwd);
                    }
                }
                if self.current.is_none()
                    && let Some(id) = self.sessions.first().map(|row| row.id.clone())
                {
                    self.select(id, cx);
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
            BridgeMsg::Event(seed, frame) => {
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
                let store = self
                    .stores
                    .iter()
                    .find(|store| store.read(cx).session_id == seed)
                    .cloned();
                let Some(store) = store else {
                    return;
                };
                store.update(cx, |store, cx| {
                    let previous_seq = store.seq;
                    crate::reducer::apply_frame(store, frame);
                    if store.seq > previous_seq && let Some(turn) = review_turn {
                        crate::code_panel::auto_review(store, turn);
                    }
                    cx.notify();
                });
                if refresh_todos {
                    let _ = self
                        .requests
                        .unbounded_send(BridgeRequest::QueryTodo { seed });
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
                        crate::reducer::clear_transient(store);
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
                    crate::reducer::apply_snapshot(store, *state);
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
            BridgeMsg::Liveness => {}
            BridgeMsg::Created(seed, cwd) => {
                self.lines.push(format!("会话已创建:{seed}"));
                self.new_session_open = false;
                self.creating_session = false;
                self.select(seed, cx);
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
                        crate::code_panel::finish_review(store, turn, request, results);
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
                        crate::reducer::clear_transient(store);
                        cx.notify();
                    });
                    let _ = self.requests.unbounded_send(BridgeRequest::Refill(seed));
                    self.lines.push("取消超时:已本地收口并重灌快照".into());
                }
            }
        }
    }

    /// 新建会话:cwd 取所选工作区路径;创建成功经 BridgeMsg::Created 自动选中。
    pub fn create_session(&mut self) {
        if self.creating_session {
            return;
        }
        let Some(model_selection) = self.new_model.clone() else { return; };
        let cwd = self
            .workspaces
            .get(self.new_workspace)
            .map(|workspace| workspace.path.clone());
        self.creating_session = self.requests.unbounded_send(BridgeRequest::CreateSession {
            cwd,
            model_selection,
        }).is_ok();
    }

    pub fn select(&mut self, seed: String, cx: &mut Context<Self>) {
        // 已访问过的会话直接复用 store(状态/流保持);新会话才 attach。
        let existing = self
            .stores
            .iter()
            .find(|store| store.read(cx).session_id == seed)
            .cloned();
        match existing {
            Some(store) => {
                let _ = self.requests.unbounded_send(BridgeRequest::QuerySessionSettings(seed));
                self.current = Some(store);
                cx.notify();
            }
            None => {
                let requests = self.requests.clone();
                let store = cx.new(|cx| SessionStore::new(seed.clone(), requests, cx));
                store.update(cx, |store, _| {
                    store.catalog = self.providers.clone();
                });
                if let Some(cwd) = self.sessions.iter().find(|row| row.id == seed)
                    .and_then(|row| row.cwd.as_deref())
                {
                    store.update(cx, |store, _| store.code_panel.cwd = Some(cwd.into()));
                }
                self.stores.push(store.clone());
                self.current = Some(store);
                let _ = self.requests.unbounded_send(BridgeRequest::Select(seed));
                cx.notify();
            }
        }
    }
}
