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
use qaqh_ringing_contract::state::InteractionView;
use qaqh_ringing_contract::state::SubagentView;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_ringing_contract::state::TurnView;
use qaqh_ringing_contract::workspace::WorkspaceEntry;

use crate::bridge::BridgeMsg;
use crate::bridge::BridgeRequest;
use gpui_component::input::{InputEvent, InputState, TextareaState};
use gpui_component::WindowExt;
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

/// ask 卡草稿:每题已选项 + 自定义输入框(焦点由 InputState 自管)。
pub struct AskDraft {
    /// 每题点选的选项文本(与 customs 互斥生效)。
    pub answers: Vec<Option<String>>,
    /// 每题自定义输入框(非空时优先于 answers)。
    pub customs: Vec<Entity<InputState>>,
}

pub struct SessionStore {
    pub snapshot_pending: bool,
    pub buffered_frames: Vec<qaqh_ringing_contract::events::SessionEventFrame>,
    pub outbox: Option<crate::delivery::Outbox>,
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
    /// composer 状态(M2):组件库 Textarea(auto_grow,Enter 提交,Shift+Enter 换行)。
    pub composer: Entity<TextareaState>,
    /// 重命名草稿(Some = 重命名模式)。
    pub renaming: Option<Entity<InputState>>,
    /// ask 卡草稿,按 interaction_id。
    pub ask_drafts: Vec<(InteractionId, AskDraft)>,
    /// ask 卡未答提示(上次提交被拒时显示,成功提交/跳过/解决后清除)。
    pub ask_hint: Option<String>,
    requests: UnboundedSender<BridgeRequest>,
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
        draft: crate::workbench::Draft,
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
            output_budget: crate::settings::get(cx).behavior.output_tail_max,
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
            composer,
            renaming: None,
            ask_drafts: Vec::new(),
            ask_hint: None,
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
            crate::delivery::DeliveryState::Sending | crate::delivery::DeliveryState::Accepted | crate::delivery::DeliveryState::Uncertain)) {
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
        let outbox = crate::delivery::Outbox {
            command_id: qaqh_ringing_contract::ulid::generate_ulid(),
            message: qaqh_client::ConversationSendMessage {
                text, images: self.images.clone(), attachments: Some(self.attachments.clone()),
                message_id: None, input_purpose: Default::default(), as_system: false,
                inter_agent: None, subagent_terminal: None,
            },
            state: crate::delivery::DeliveryState::Sending, detail: String::new(),
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

    pub fn draft(&self, cx: &App) -> crate::workbench::Draft {
        crate::workbench::Draft {
            text: self.composer.read(cx).value().to_string(), images: self.images.clone(),
            attachments: self.attachments.clone(), outbox: self.outbox.clone(),
            local_root: self.code_panel.local_root(),
        }
    }
    pub fn persist_draft(&self, cx: &App) -> Result<(), String> {
        let mut state = crate::workbench::load()?;
        state.drafts.insert(self.session_id.clone(), self.draft(cx));
        crate::workbench::save(&state)
    }
    pub fn retry_message(&mut self) {
        if let Some(outbox) = &mut self.outbox {
            outbox.state = crate::delivery::DeliveryState::Sending;
            if self.requests.unbounded_send(BridgeRequest::SendMessage { seed: self.session_id.clone(), outbox: outbox.clone() }).is_err() {
                outbox.state = crate::delivery::DeliveryState::Uncertain;
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

    pub fn respond_plan(&mut self, interaction_id: String, approved: bool) {
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::RespondPlan {
                seed: self.session_id.clone(),
                interaction_id,
                approved,
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
    pub archived: bool,
}

fn rows_from(entries: &[SessionListEntry]) -> Vec<SessionRow> {
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

fn todos_from(value: &qaqh_ringing_contract::todo::TodoListSnapshot) -> Vec<TodoRow> {
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
    fn poll_request(&self) -> BridgeRequest {
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
    pub connection: crate::bridge::ConnectionState,
    pub workbench: crate::workbench::Workbench,
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
    pub requests: UnboundedSender<BridgeRequest>,
}

impl DaemonStore {
    pub fn new(
        requests: UnboundedSender<BridgeRequest>,
        workbench: crate::workbench::Workbench,
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
                crate::settings::update(cx, |content| content.typography.ui_font_family = text);
            }
        })
        .detach();
        Self {
            last_operation: None,
            connection: crate::bridge::ConnectionState::Connecting,
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
            requests,
        }
    }

    pub fn handle(&mut self, message: BridgeMsg, window: &mut Window, cx: &mut Context<Self>) {
        match message {
            BridgeMsg::FromConnection(generation, message) => {
                if generation == crate::bridge::connection_generation() { self.handle(*message, window, cx); }
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
                        crate::diagnostics::record(if ack.status == qaqh_client::RingingCommandAckStatus::Accepted { "command_accepted" } else { "command_rejected" },
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
                let id = match &state { crate::bridge::ConnectionState::Ready { client_session_id } => Some(client_session_id.clone()), _ => None };
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
                            let was_pending = matches!(outbox.state, crate::delivery::DeliveryState::Sending | crate::delivery::DeliveryState::Uncertain);
                            match result {
                                crate::bridge::DeliveryResult::Ack(ack) => outbox.acknowledge(&ack),
                                crate::bridge::DeliveryResult::Receipt(status) => outbox.receipt(&status),
                                crate::bridge::DeliveryResult::Unknown(detail) => {
                                    outbox.state = crate::delivery::DeliveryState::Uncertain;
                                    outbox.detail = detail;
                                }
                            }
                            if was_pending && matches!(outbox.state, crate::delivery::DeliveryState::Accepted | crate::delivery::DeliveryState::Succeeded)
                                && store.composer.read(cx).value().trim() == outbox.message.text {
                                store.composer.update(cx, |composer, cx| composer.set_value("", window, cx));
                                store.images.clear(); store.attachments.clear();
                            }
                            if outbox.state == crate::delivery::DeliveryState::Succeeded {
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
                            Ok(crate::bridge::PreparedAttachment::Image(image)) => store.images.push(image),
                            Ok(crate::bridge::PreparedAttachment::Content(reference)) => store.attachments.push(reference),
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
                            Ok(text) => crate::content::replace(store, &reference, text),
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
                    crate::reducer::apply_frame(store, frame, cx);
                    if store.seq > previous_seq && let Some(title) = notification { crate::desktop::notify(cx, &store.session_id, title); }
                    if store.seq > previous_seq && let Some(turn) = review_turn {
                        crate::code_panel::auto_review(store, turn);
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
                    crate::ui::open_ask_dialog(entity, iid, window, cx);
                }
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
                        store.snapshot_pending = true;
                        store.buffered_frames.clear();
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
                    crate::diagnostics::record("snapshot", Some(&store.session_id), None, Some(store.seq));
                    store.snapshot_pending = false;
                    for frame in std::mem::take(&mut store.buffered_frames) { crate::reducer::apply_frame(store, frame, cx); }
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
                        crate::bridge::ConnectionState::Ready { client_session_id } => Some(client_session_id.clone()), _ => None,
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
        if let Err(error) = crate::workbench::save(&self.workbench) {
            if self.lines.last() != Some(&error) { self.lines.push(error); }
        }
    }
}
