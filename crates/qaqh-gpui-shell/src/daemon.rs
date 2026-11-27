//! GPUI 实体状态(M3):字段直持 RingingV2 契约类型,零前端 DTO。
//! 多会话:visited store 常驻缓存、事件按 seed 路由;输入框统一走
//! gpui-component 的 InputState/TextareaState(InputState::new 需要
//! &mut Window,故实体构造一律在窗口上下文内完成)。

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use futures::channel::mpsc::UnboundedSender;
use gpui_kit::AppContext as _;
use gpui_kit::Context;
use gpui_kit::Entity;
use gpui_kit::Subscription;
use gpui_kit::Window;
use qaqh_ringing_contract::events::SessionEvent;
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::workspace::WorkspaceEntry;

use qaqh_gpui_wire::bridge::{
    BridgeMsg, BridgeRequest, ProviderPresetView, ShellInfo, SkillsScanReport,
};
use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::component::WindowExt;
use qaqh_profile::{ConfigDto, ProfilesDocument};
use qaqh_types::{CredentialSource, ModelSelection};
use qaqh_gpui_session::store::{SessionStore, SessionRow, TodoRow, SettingsEditor, AccountService, AccountSignIn, StreamState, rows_from, todos_from, model_options_from};
use crate::shell_panel::ShellTabState;
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

/// 主导航分区(对齐 Overcoder v5.0: 主页 / 工具 / 团队 / 接入)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrimarySection {
    #[default]
    Home,
    Tools,
    Team,
    Access,
}

/// 「工具」页子分栏(Skills / MCP)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolsTab {
    #[default]
    Skills,
    Mcp,
}

/// 全局策略页行内输入框实体集合:改完回车或点应用即写 daemon,告别整页切表单。
pub struct GlobalConfigInputs {
    pub tool_allowlist: Entity<InputState>,
    pub auto_compact: Entity<InputState>,
    pub idle_unload: Entity<InputState>,
    pub default_shell: Entity<InputState>,
    pub goal_budget: Entity<InputState>,
    pub seeded: bool,
}

/// 侧栏右键菜单目标:会话行或工作区标题行。
pub enum SidebarCtxTarget {
    Session {
        session_id: String,
        archived: bool,
    },
    Workspace {
        workspace_id: String,
        title: String,
        archived_view: bool,
        session_ids: Vec<String>,
    },
}

/// 侧栏右键菜单状态:目标对象 + 打开位置;删除统一走二段确认。
pub struct SidebarCtxMenu {
    pub target: SidebarCtxTarget,
    pub position: gpui_kit::Point<gpui_kit::Pixels>,
    pub confirm_delete: bool,
}

/// 侧栏行内重命名目标:同一时刻最多激活一个重命名输入框。
#[derive(Clone, PartialEq, Eq)]
pub enum RenameTarget {
    Session(String),
    Workspace(String),
}

/// 窗口级跨区飞箱状态:A 卡头盒槽 → 侧栏后继会话行。
pub struct FlyBoxState {
    pub from: gpui_kit::Point<gpui_kit::Pixels>,
    pub target_session_id: String,
    pub started_at: std::time::Instant,
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
    /// 侧栏工作区分组展开全部会话的组键(未在集合中时默认展示前 5 条)。
    pub expanded_more: HashSet<String>,
    /// 侧栏右键菜单(会话行/工作区行右键打开;None = 关闭)。
    pub ctx_menu: Option<SidebarCtxMenu>,
    /// 侧栏行内重命名草稿(目标, 输入框实体)。
    pub rename_draft: Option<(RenameTarget, Entity<InputState>)>,
    /// 设置页打开状态(占用主区,侧栏常驻)。
    pub settings_open: bool,
    /// 设置页 daemon 侧数据:config.load 快照 + provider.list + 官方预设表。
    pub config: Option<ConfigDto>,
    pub providers: Option<Arc<ProfilesDocument>>,
    pub provider_presets: Vec<ProviderPresetView>,
    pub new_model: Option<ModelSelection>,
    /// 主导航当前分区与页内状态(Overcoder v5.0: 主页/工具/团队/接入)。
    pub section: PrimarySection,
    pub page_epoch: u64,
    pub tools_tab: ToolsTab,
    pub skills_scanning: bool,
    pub skills_report: Option<SkillsScanReport>,
    pub expanded_skills: HashSet<String>,
    pub team_page_seed: Option<String>,
    pub team_page_snapshot: Option<(String, qaqh_client::v2::TeamResponse)>,
    pub team_page_loading: bool,
    pub team_poll_tick: u8,
    /// 内联启动页(StartupView)双层 Composer 输入状态与预检工具档位。
    pub startup_input: Entity<TextareaState>,
    pub startup_tool_mode: Option<String>,
    /// 新建会话/内联启动页激活状态 + 选择(workspace 下标)。
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
    pub ds_bonuses: Vec<qaqh_client::service_types::DsAccountBonus>,
    /// DeepSeek 账号模型同步状态与首登/启动自检标记。
    pub ds_models_syncing: bool,
    ds_auto_synced: bool,
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
    /// 用户 shell 终端坞(ADR-0028):标签/进程生命周期归 store,不归 DOM;
    /// 隐藏坞/切会话不断流,只有显式关标签才 shell.close(§D5 断联不 kill)。
    pub shell_dock_open: bool,
    pub shells: Vec<ShellInfo>,
    pub shell_active: Option<String>,
    pub shell_tabs: HashMap<String, ShellTabState>,
    /// 上次用的档位(「+」直开沿用;Windows Terminal 同律)。
    pub shell_last_kind: &'static str,
    pub shell_opening: Option<&'static str>,
    pub shell_error: Option<String>,
    /// 工作台面板收折态(弹簧动画驱动)。
    pub sidebar_open: bool,
    /// 因右侧二分抽屉（代码面板或后台执行观察窗）展开而自动收起侧栏;关闭或切走时按原样恢复。
    pub sidebar_restore_on_side_panel_close: bool,
    pub inspector_open: bool,
    /// 工作区页行内路径输入框。
    pub workspace_path_input: Entity<InputState>,
    /// 全局策略页行内输入框。
    pub global_inputs: GlobalConfigInputs,
    /// 跨组件交接飞箱坐标锚点表。
    pub handoff_anchors: qaqh_gpui_session::handoff::HandoffAnchors,
    /// 正在播放的跨区飞箱浮层状态。
    pub fly_box: Option<FlyBoxState>,
    /// 飞箱代际令牌(防手切打断后回调误切会话)。
    pub fly_box_epoch: u64,
    /// 缓存会话的导航事件订阅(随 LRU 淘汰一起清理)。
    pub session_subs: Vec<(String, Subscription)>,
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
        let workspace_path_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("输入或浏览 daemon 所在机器的工作区目录…"));
        let global_inputs = GlobalConfigInputs {
            tool_allowlist: cx.new(|cx| InputState::new(window, cx).placeholder("逗号分隔工具名…")),
            auto_compact: cx.new(|cx| InputState::new(window, cx).placeholder("0.75")),
            idle_unload: cx.new(|cx| InputState::new(window, cx).placeholder("秒，0 为关闭")),
            default_shell: cx.new(|cx| InputState::new(window, cx).placeholder("auto")),
            goal_budget: cx.new(|cx| InputState::new(window, cx).placeholder("留空 = 不限")),
            seeded: false,
        };
        let startup_input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 7)
                .submit_on_enter(true)
                .placeholder("输入你的任务需求，或直接提问…")
        });
        cx.subscribe(&startup_input, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { shift: false, .. }) {
                this.submit_startup(cx);
            }
            cx.notify();
        })
        .detach();
        Self {
            last_operation: None,
            connection: qaqh_gpui_wire::bridge::ConnectionState::Connecting,
            collapsed: workbench.collapsed.iter().cloned().collect(),
            expanded_more: HashSet::new(),
            workbench,
            session_search,
            show_archived: false,
            ctx_menu: None,
            rename_draft: None,
            lines: vec![load_error.unwrap_or_else(|| "qaqh-gpui 启动…".into())],
            sessions: Vec::new(),
            workspaces: Vec::new(),
            settings_open: false,
            config: None,
            providers: None,
            provider_presets: Vec::new(),
            new_model: None,
            section: PrimarySection::Home,
            page_epoch: 0,
            tools_tab: ToolsTab::Skills,
            skills_scanning: false,
            skills_report: None,
            expanded_skills: HashSet::new(),
            team_page_seed: None,
            team_page_snapshot: None,
            team_page_loading: false,
            team_poll_tick: 0,
            startup_input,
            startup_tool_mode: None,
            new_session_open: false,
            creating_session: false,
            new_workspace: 0,
            font_field,
            settings_editor: SettingsEditor::default(),
            settings_form: None,
            siwc: None,
            dsaccount: None,
            ds_balance: None,
            ds_bonuses: Vec::new(),
            ds_models_syncing: false,
            ds_auto_synced: false,
            sign_in: None,
            account_line: None,
            stores: Vec::new(),
            current: None,
            todos: None,
            team: None,
            shell_dock_open: false,
            shells: Vec::new(),
            shell_active: None,
            shell_tabs: HashMap::new(),
            shell_last_kind: "pwsh",
            shell_opening: None,
            shell_error: None,
            sidebar_open: true,
            sidebar_restore_on_side_panel_close: false,
            inspector_open: false,
            workspace_path_input,
            global_inputs,
            handoff_anchors: Default::default(),
            fly_box: None,
            fly_box_epoch: 0,
            session_subs: Vec::new(),
            requests,
        }
    }

    /// 把 daemon 全局配置快照同步进设置页行内输入框。
    pub(crate) fn sync_global_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(config) = &self.config else {
            return;
        };
        let allowlist = config.tool_allowlist.join(", ");
        let compact = config.auto_compact_threshold.to_string();
        let unload = config.session_idle_unload_secs.to_string();
        let shell = config
            .exec
            .default_shell
            .as_deref()
            .unwrap_or("auto")
            .to_owned();
        let budget = config
            .goals
            .max_token_budget
            .map(|b| b.to_string())
            .unwrap_or_default();
        self.global_inputs
            .tool_allowlist
            .update(cx, |f, cx| f.set_value(&allowlist, window, cx));
        self.global_inputs
            .auto_compact
            .update(cx, |f, cx| f.set_value(&compact, window, cx));
        self.global_inputs
            .idle_unload
            .update(cx, |f, cx| f.set_value(&unload, window, cx));
        self.global_inputs
            .default_shell
            .update(cx, |f, cx| f.set_value(&shell, window, cx));
        self.global_inputs
            .goal_budget
            .update(cx, |f, cx| f.set_value(&budget, window, cx));
        self.global_inputs.seeded = true;
    }

    pub(crate) fn active_side_panel_open(&self, cx: &gpui_kit::App) -> bool {
        self.section == PrimarySection::Home
            && !self.new_session_open
            && self.current.as_ref().is_some_and(|store| {
                let s = store.read(cx);
                s.code_panel.is_open() || s.terminal_panel.open
            })
    }

    /// 联动左侧工作区列表与右侧二分面板(代码/文档面板或后台执行观察窗):
    /// 右侧面板展开时自动收回侧栏形成左右二分;收回或切离时恢复侧栏原样。
    pub(crate) fn sync_sidebar_for_side_panel(&mut self, cx: &mut Context<Self>) {
        if self.active_side_panel_open(cx) {
            if self.sidebar_open {
                self.sidebar_open = false;
                self.sidebar_restore_on_side_panel_close = true;
                cx.notify();
            }
        } else if self.sidebar_restore_on_side_panel_close {
            self.sidebar_open = true;
            self.sidebar_restore_on_side_panel_close = false;
            cx.notify();
        }
    }

    pub(crate) fn toggle_sidebar_manual(&mut self, open: Option<bool>, cx: &mut Context<Self>) {
        self.sidebar_restore_on_side_panel_close = false;
        self.sidebar_open = open.unwrap_or(!self.sidebar_open);
        cx.notify();
    }

    /// 切换主导航分区(主页 / 工具 / 团队 / 接入)并自动触发对应数据加载。
    pub fn navigate(
        &mut self,
        section: PrimarySection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.section != section || self.settings_open {
            self.page_epoch = self.page_epoch.wrapping_add(1);
        }
        self.section = section;
        self.settings_open = false;
        self.settings_form = None;
        match section {
            PrimarySection::Home => {
                self.new_session_open = false;
                if self.current.is_none() {
                    let _ = self.requests.unbounded_send(BridgeRequest::ListProviders);
                    self.startup_input
                        .update(cx, |field, cx| field.focus(window, cx));
                }
            }
            PrimarySection::Tools => {
                self.new_session_open = false;
                let _ = self.requests.unbounded_send(BridgeRequest::QueryConfig);
                self.refresh_skills();
            }
            PrimarySection::Team => {
                self.new_session_open = false;
                if self.team_page_seed.is_none() {
                    self.team_page_seed = self
                        .current
                        .as_ref()
                        .map(|c| c.read(cx).session_id.clone())
                        .or_else(|| self.sessions.iter().find(|s| !s.archived).map(|s| s.id.clone()));
                }
                self.refresh_team_page();
            }
            PrimarySection::Access => {
                self.new_session_open = false;
                self.refresh_accounts();
                let _ = self.requests.unbounded_send(BridgeRequest::ListProviders);
                let _ = self.requests.unbounded_send(BridgeRequest::QueryConfig);
            }
        }
        self.sync_sidebar_for_side_panel(cx);
        cx.notify();
    }

    /// 打开内联 StartupView 准备新建对话,并聚焦其双层 Composer。
    pub fn open_startup(
        &mut self,
        workspace_idx: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(idx) = workspace_idx {
            self.new_workspace = idx;
        }
        if !self.new_session_open || self.section != PrimarySection::Home || self.settings_open {
            self.page_epoch = self.page_epoch.wrapping_add(1);
        }
        self.section = PrimarySection::Home;
        self.new_session_open = true;
        self.settings_open = false;
        self.settings_form = None;
        let _ = self.requests.unbounded_send(BridgeRequest::ListProviders);
        self.startup_input
            .update(cx, |field, cx| field.focus(window, cx));
        self.sync_sidebar_for_side_panel(cx);
        cx.notify();
    }

    /// 跨工作区刷新 daemon `skills.catalog`。
    pub fn refresh_skills(&mut self) {
        self.skills_scanning = true;
        let workspaces = self
            .workspaces
            .iter()
            .map(|w| (Some(w.path.clone()), w.title.clone()))
            .collect();
        let _ = self
            .requests
            .unbounded_send(BridgeRequest::QuerySkills { workspaces });
    }

    /// 刷新「团队」页当前选定会话的 team 快照。
    pub fn refresh_team_page(&mut self) {
        if let Some(seed) = self.team_page_seed.clone() {
            self.team_page_loading = true;
            let _ = self
                .requests
                .unbounded_send(BridgeRequest::QueryTeam { seed });
        }
    }

    /// 后台 2s 心跳调入:团队页可见时约每 6s 低频轮询刷新。
    pub fn poll_team_page(&mut self) {
        if self.section == PrimarySection::Team && self.team_page_seed.is_some() {
            self.team_poll_tick = self.team_poll_tick.wrapping_add(1);
            if self.team_poll_tick.is_multiple_of(3) {
                self.refresh_team_page();
            }
        }
    }

    /// 打开设置页(种字体输入框与全局策略行内输入框 + 拉配置/服务商目录)。
    pub(crate) fn open_settings(
        &mut self,
        editor: SettingsEditor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.settings_open || self.settings_editor != editor {
            self.settings_open = true;
            self.new_session_open = false;
            self.settings_editor = editor;
            self.settings_form = None;
            let family = qaqh_gpui_kit::settings::get(cx)
                .typography
                .ui_font_family
                .clone();
            self.font_field
                .update(cx, |field, cx| field.set_value(&family, window, cx));
            self.sync_global_inputs(window, cx);
            let _ = self.requests.unbounded_send(BridgeRequest::QueryConfig);
            let _ = self.requests.unbounded_send(BridgeRequest::ListProviders);
            if self.provider_presets.is_empty() {
                let _ = self
                    .requests
                    .unbounded_send(BridgeRequest::ListProviderPresets);
            }
            if matches!(
                editor,
                SettingsEditor::Accounts | SettingsEditor::Catalog
            ) {
                self.refresh_accounts();
            }
        }
        cx.notify();
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
                    "conversation" => "对话操作", "exec_session" => "执行会话操作",
                    _ => "操作",
                };
                match result {
                    Ok(ack) => {
                        qaqh_gpui_wire::diagnostics::record(if ack.status == qaqh_client::RingingCommandAckStatus::Accepted { "command_accepted" } else { "command_rejected" },
                            match &target { qaqh_client::CommandTarget::Session { session_id } => Some(session_id.as_str()), _ => None }, Some(&ack.command_id), None);
                        if ack.status == qaqh_client::RingingCommandAckStatus::Rejected {
                            self.last_operation = Some(format!("{label} · 被拒绝 · {} {}", ack.code.unwrap_or_default(), ack.message.unwrap_or_default()));
                        } else {
                            self.last_operation = None;
                            // 成功受理走瞬时气泡;常驻状态行只承载失败/待确认。
                            let status = if ack.result.is_some() { "已完成" } else { "已受理" };
                            window.push_notification(
                                gpui_kit::component::notification::Notification::success(
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
                if id.is_some() {
                    self.shell_error = None;
                    self.refresh_accounts();
                    if self.shell_dock_open {
                        let _ = self.requests.unbounded_send(BridgeRequest::ShellList);
                    }
                }
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
            BridgeMsg::Line(text) => {
                self.lines.push(text);
                if self.lines.len() > 200 { self.lines.drain(..100); }
                cx.notify();
            }
            BridgeMsg::Sessions(value) => {
                crate::desktop::roster_notifications(&self.sessions, &value, cx);
                self.sessions = rows_from(&value);
                for store in &self.stores {
                    let id = store.read(cx).session_id.clone();
                    if let Some(row) = self.sessions.iter().find(|row| row.id == id) {
                        let cwd = row.cwd.as_deref().map(Into::into);
                        store.update(cx, |store, _| store.code_panel.cwd = cwd);
                    }
                    self.sync_store_lineage(store, cx);
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
                if self.section == PrimarySection::Tools {
                    self.refresh_skills();
                }
                cx.notify();
            }
            BridgeMsg::SkillsCatalog(report) => {
                self.skills_scanning = false;
                self.skills_report = Some(report);
                cx.notify();
            }
            BridgeMsg::Todos(seed, value) => {
                let rows = todos_from(&value);
                for store in &self.stores {
                    if store.read(cx).session_id == seed {
                        let rows = rows.clone();
                        store.update(cx, |s, cx| {
                            s.todos = rows;
                            cx.notify();
                        });
                    }
                }
                let current_seed = self
                    .current
                    .as_ref()
                    .map(|current| current.read(cx).session_id.clone());
                if current_seed.as_deref() == Some(seed.as_str()) {
                    self.todos = Some((seed, rows));
                    cx.notify();
                }
            }
            BridgeMsg::Team(seed, value) => {
                // 对当前激活会话侧栏 & 独立「团队」页分别更新;花名册是粗投影。
                let current_seed = self
                    .current
                    .as_ref()
                    .map(|current| current.read(cx).session_id.clone());
                let matches_page = self.team_page_seed.as_deref() == Some(seed.as_str());
                let mut changed = false;
                if current_seed.as_deref() == Some(seed.as_str()) {
                    if matches_page {
                        self.team_page_snapshot = Some((seed.clone(), (*value).clone()));
                        self.team_page_loading = false;
                    }
                    self.team = Some((seed, *value));
                    changed = true;
                } else if matches_page {
                    self.team_page_snapshot = Some((seed, *value));
                    self.team_page_loading = false;
                    changed = true;
                }
                if changed {
                    cx.notify();
                }
            }
            BridgeMsg::Event(seed, frame) => {
                let notification = match &frame.event {
                    SessionEvent::InteractionRequested { .. } => Some("会话等待你的响应"),
                    SessionEvent::TurnEnded { outcome, .. } => Some(match outcome {
                        qaqh_ringing_contract::state::TurnOutcome::Finished { error: Some(_), .. } => "本轮工作失败，请查看会话",
                        _ => "本轮工作已结束",
                    }),
                    _ => None,
                };
                let is_foreground = self
                    .current
                    .as_ref()
                    .is_some_and(|store| store.read(cx).session_id == seed);
                let review_turn = match &frame.event {
                    SessionEvent::TurnEnded { turn_id, .. } if is_foreground => {
                        Some(turn_id.clone())
                    }
                    _ => None,
                };
                let live_patch_event = if is_foreground {
                    match &frame.event {
                        SessionEvent::ToolArgumentsDelta {
                            call_id, tool_name, ..
                        } if tool_name == "apply_patch" => Some(
                            qaqh_gpui_session::code_panel::LivePatchEvent::Delta(call_id.clone()),
                        ),
                        SessionEvent::ToolFinished {
                            call_id, outcome, ..
                        } => Some(qaqh_gpui_session::code_panel::LivePatchEvent::Finished {
                            call_id: call_id.clone(),
                            success: outcome.is_success(),
                        }),
                        _ => None,
                    }
                } else {
                    None
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
                    if store.seq > previous_seq
                        && let Some(event) = live_patch_event
                    {
                        qaqh_gpui_session::code_panel::on_live_patch_event(store, event, cx);
                    }
                    if store.seq > previous_seq && let Some(title) = notification { crate::desktop::notify(cx, &store.session_id, title); }
                    if store.seq > previous_seq && let Some(turn) = review_turn {
                        qaqh_gpui_session::code_panel::auto_review(store, turn, cx);
                    }
                    cx.notify();
                });
                // ADR-0016 自动接续:封箱线正开着且模型未声明「不拽我」→ 先播飞箱再切后继
                // (HandoffSealed 权威事件驱动,不看本地钟;后台会话封箱不打扰)。
                if let Some(current) = self.current.as_ref()
                    && current.read(cx).session_id == seed
                {
                    let jump = current.read(cx).handoff_auto_jump.clone();
                    if let Some(successor) = jump {
                        current.update(cx, |store, _| store.handoff_auto_jump = None);
                        self.jump_with_fly(successor, Some(seed.clone()), window, cx);
                    }
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
                    qaqh_gpui_session::reducer::sync_handoff_after_snapshot(store, cx);
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
                self.section = PrimarySection::Home;
                self.new_session_open = false;
                self.creating_session = false;
                self.startup_input
                    .update(cx, |field, cx| field.set_value("", window, cx));
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
                self.sync_global_inputs(window, cx);
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
                self.maybe_auto_sync_dsaccount();
                cx.notify();
            }
            BridgeMsg::ProviderPresets(presets) => {
                self.provider_presets = presets;
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
            BridgeMsg::Shells(shells) => crate::shell_panel::sync_shells(self, shells, cx),
            BridgeMsg::ShellOpened(info) => crate::shell_panel::opened(self, info, window, cx),
            BridgeMsg::ShellRemoved(shell_id) => crate::shell_panel::removed(self, &shell_id, cx),
            BridgeMsg::ShellBytes { shell_id, bytes } => {
                crate::shell_panel::on_bytes(self, &shell_id, &bytes, cx);
            }
            BridgeMsg::ShellStatus { shell_id, status } => {
                crate::shell_panel::on_status(self, &shell_id, status, cx);
            }
            BridgeMsg::ShellError(error) => {
                crate::shell_panel::on_error(self, error, window, cx);
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

    pub fn siwc_ready(&self) -> bool {
        self.siwc.as_ref().is_some_and(|s| {
            s.status == qaqh_client::service_types::SiwcConnectionState::Connected && s.sharing
        })
    }

    pub fn dsaccount_signed_in(&self) -> bool {
        self.dsaccount.as_ref().is_some_and(|s| s.signed_in)
    }

    fn sync_account_readiness(&self, cx: &mut Context<Self>) {
        let siwc_ready = self.siwc_ready();
        let ds_ready = self.dsaccount_signed_in();
        for store in &self.stores {
            store.update(cx, |store, cx| {
                if store.siwc_ready != siwc_ready || store.dsaccount_signed_in != ds_ready {
                    store.siwc_ready = siwc_ready;
                    store.dsaccount_signed_in = ds_ready;
                    cx.notify();
                }
            });
        }
    }

    fn has_dsaccount_models(&self) -> bool {
        self.providers.as_deref().is_some_and(|catalog| {
            catalog.providers.values().any(|provider| {
                provider.credential == CredentialSource::DeepseekAccount
                    && !provider.models.is_empty()
            })
        })
    }

    fn maybe_auto_sync_dsaccount(&mut self) {
        if self.dsaccount_signed_in()
            && self.providers.is_some()
            && !self.ds_auto_synced
            && !self.ds_models_syncing
            && !self.has_dsaccount_models()
        {
            self.ds_auto_synced = true;
            self.sync_dsaccount_models();
        }
    }

    /// 浏览器登录入口:起流程并打开授权页;轮询由 2s 定时器驱动。
    pub fn sign_in_start(&mut self, service: AccountService) {
        if self.sign_in.as_ref().is_some_and(|flow| flow.service != service) {
            self.sign_in_cancel();
        }
        let (method, params) = match service {
            AccountService::Siwc => (
                "siwc.sign_in.start",
                serde_json::Value::Object(Default::default()),
            ),
            AccountService::Dsaccount => (
                "dsaccount.sign_in.start",
                serde_json::json!({ "locale": "zh" }),
            ),
        };
        let _ = self.requests.unbounded_send(BridgeRequest::Service {
            method: method.into(),
            params,
        });
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

    pub fn verify_dsaccount(&mut self) {
        self.account_line = Some("正在校验 DeepSeek 账号凭据…".into());
        self.service("dsaccount.verify");
    }

    pub fn ack_dsaccount_bonus(&mut self, order_id: String) {
        self.ds_bonuses.retain(|bonus| bonus.order_id != order_id);
        let _ = self.requests.unbounded_send(BridgeRequest::Service {
            method: "dsaccount.bonus.ack".into(),
            params: serde_json::json!({ "order_id": order_id }),
        });
    }

    pub fn sync_dsaccount_models(&mut self) {
        self.last_operation = None;
        self.ds_models_syncing = true;
        self.service("dsaccount.models");
    }

    /// 2s 定时器:登录流程轮询 + 过期收口(同时向后端发送 cancel 关闭回环监听)。
    pub fn poll_sign_in(&mut self) {
        let Some(flow) = &self.sign_in else { return };
        if now_ms() > flow.expires_at_ms {
            self.sign_in_cancel();
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
            let error = result.unwrap_err();
            if method == "dsaccount.models" {
                self.ds_models_syncing = false;
                self.last_operation = Some(format!("同步 DeepSeek 模型失败：{error}"));
            }
            self.account_line = Some(error);
            return;
        };
        match method {
            "siwc.status" => match decode(method, value) {
                Ok(status) => {
                    self.siwc = Some(status);
                    self.sync_account_readiness(cx);
                }
                Err(error) => self.account_line = Some(error),
            },
            "dsaccount.status" => {
                match decode::<st::DsAccountStatus>(method, value) {
                    Ok(status) => {
                        let signed_in = status.signed_in;
                        self.dsaccount = Some(status);
                        if signed_in {
                            self.service("dsaccount.balance");
                            self.service("dsaccount.bonuses");
                            self.maybe_auto_sync_dsaccount();
                        } else {
                            self.ds_balance = None;
                            self.ds_bonuses.clear();
                            self.ds_auto_synced = false;
                            self.ds_models_syncing = false;
                        }
                        self.sync_account_readiness(cx);
                    }
                    Err(error) => self.account_line = Some(error),
                }
            }
            "dsaccount.balance" => match decode(method, value) {
                Ok(balance) => self.ds_balance = Some(balance),
                Err(error) => self.account_line = Some(error),
            },
            "dsaccount.bonuses" => match decode(method, value) {
                Ok(bonuses) => self.ds_bonuses = bonuses,
                Err(error) => self.account_line = Some(error),
            },
            "dsaccount.bonus.ack" => {
                self.service("dsaccount.bonuses");
            }
            "dsaccount.verify" => match decode::<st::DsAccountVerify>(method, value) {
                Ok(verify) if verify.valid => {
                    self.account_line = Some("DeepSeek 账号凭据验证通过".into());
                }
                Ok(verify) if verify.cleared => {
                    self.ds_balance = None;
                    self.ds_bonuses.clear();
                    self.service("dsaccount.status");
                    self.account_line = Some(match verify.reason {
                        Some(reason) => format!("DeepSeek 凭据已失效并清除：{reason}"),
                        None => "DeepSeek 凭据已失效并清除，请重新登录".into(),
                    });
                }
                Ok(verify) => {
                    self.account_line = Some(match verify.reason {
                        Some(reason) => format!("DeepSeek 凭据暂未验证通过：{reason}"),
                        None => "DeepSeek 凭据暂未验证通过".into(),
                    });
                }
                Err(error) => self.account_line = Some(error),
            },
            "dsaccount.models" => {
                self.ds_models_syncing = false;
                match decode::<Vec<st::DsAccountModel>>(method, value) {
                    Ok(models) => {
                        let _ = self.requests.unbounded_send(BridgeRequest::ListProviders);
                        self.account_line = Some(format!(
                            "已同步 {} 个 DeepSeek 账号模型",
                            models.len()
                        ));
                    }
                    Err(error) => {
                        self.last_operation = Some(error.clone());
                        self.account_line = Some(error);
                    }
                }
            }
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
                        let _ = self.requests.unbounded_send(BridgeRequest::ListProviders);
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
                    Ok(status) => {
                        if let Some(current) = &self.sign_in
                            && current.service == AccountService::Dsaccount
                            && let Some(attempt_id) = status.attempt_id.as_deref()
                            && attempt_id != current.flow_id
                        {
                            self.sign_in = None;
                            return;
                        }
                        match status.state {
                            st::DsFlowState::SignedIn => {
                                self.sign_in = None;
                                self.ds_balance = None;
                                self.ds_auto_synced = true;
                                self.service("dsaccount.status");
                                let _ = self.requests.unbounded_send(BridgeRequest::ListProviders);
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
                        }
                    }
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
                self.ds_bonuses.clear();
                self.ds_auto_synced = false;
                self.ds_models_syncing = false;
                self.sync_account_readiness(cx);
                self.service("dsaccount.status");
                self.account_line = Some("已登出 DeepSeek 账号".into());
            }
            _ => {}
        }
    }

    /// 新建会话:cwd 取所选工作区路径;创建成功经 BridgeMsg::Created 自动选中。
    /// 模型未选 = None(继承全局默认,eb3f31c 的 model_selection Inherit 口径)。
    pub fn create_session(&mut self) {
        self.create_session_with_prompt(None);
    }

    pub fn create_session_with_prompt(&mut self, first_message: Option<String>) {
        if self.creating_session {
            return;
        }
        let cwd = self
            .workspaces
            .get(self.new_workspace)
            .map(|workspace| workspace.path.clone());
        self.creating_session = self
            .requests
            .unbounded_send(BridgeRequest::CreateSession {
                cwd,
                model_selection: self.new_model.clone(),
                tool_mode: self.startup_tool_mode.clone(),
                title: None,
                first_message,
                handoff_from: None,
            })
            .is_ok();
    }

    pub fn submit_startup(&mut self, cx: &mut Context<Self>) {
        let text = self.startup_input.read(cx).value().trim().to_string();
        if text.is_empty() || self.creating_session {
            return;
        }
        self.create_session_with_prompt(Some(text));
        cx.notify();
    }

    fn sync_store_lineage(&self, store: &Entity<SessionStore>, cx: &mut Context<Self>) {
        let store_id = store.read(cx).session_id.clone();
        let meta_info = self
            .sessions
            .iter()
            .find(|row| row.id == store_id)
            .map(|row| (row.handoff_from_session_id.clone(), row.created_at_ms));
        let child_seal = self
            .sessions
            .iter()
            .find(|row| row.handoff_from_session_id.as_deref() == Some(store_id.as_str()))
            .map(|child| qaqh_gpui_session::store::HandoffSeal {
                sealed_session_id: store_id.clone(),
                successor_session_id: child.id.clone(),
                work_summary: clean_session_title(child, &self.workspaces).to_string(),
                at_ms: child.created_at_ms,
            });
        store.update(cx, |store, cx| {
            let mut changed = false;
            if let Some((parent_id, created_at_ms)) = meta_info {
                if store.handoff_from_session_id != parent_id {
                    store.handoff_from_session_id = parent_id;
                    changed = true;
                }
                if store.session_created_at_ms != created_at_ms && created_at_ms > 0 {
                    store.session_created_at_ms = created_at_ms;
                    changed = true;
                }
            }
            if store.handoff_seal.is_none()
                && let Some(seal) = child_seal
            {
                store.handoff_seal = Some(seal);
                changed = true;
            }
            if changed {
                cx.notify();
            }
        });
    }

    /// 交接跨区跳转:Full 动效下先从 A 侧盒槽飞箱 680ms 落位至侧栏目标行，再切到后继会话。
    pub fn jump_with_fly(
        &mut self,
        target_session_id: String,
        fly_from_session: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.requests.unbounded_send(BridgeRequest::RefreshRoster);
        let full_motion = qaqh_gpui_kit::motion::effective_motion(cx)
            == qaqh_gpui_kit::settings::MotionLevel::Full;
        if full_motion
            && let Some(from_id) = fly_from_session
            && let Some(from_bounds) = self.handoff_anchors.seal_icon(&from_id)
        {
            self.fly_box_epoch = self.fly_box_epoch.wrapping_add(1);
            let epoch = self.fly_box_epoch;
            self.fly_box = Some(FlyBoxState {
                from: from_bounds.center(),
                target_session_id: target_session_id.clone(),
                started_at: std::time::Instant::now(),
            });
            cx.notify();
            let dur = std::time::Duration::from_secs_f32(
                qaqh_gpui_session::handoff::FLY_BOX_SECS,
            );
            cx.spawn_in(window, async move |daemon, cx| {
                cx.background_executor().timer(dur).await;
                let _ = daemon.update_in(cx, |daemon, window, cx| {
                    if daemon.fly_box_epoch == epoch {
                        daemon.fly_box = None;
                        daemon.select(target_session_id, window, cx);
                    }
                });
            })
            .detach();
            return;
        }
        self.select(target_session_id, window, cx);
    }

    pub fn select(&mut self, seed: String, window: &mut Window, cx: &mut Context<Self>) {
        self.fly_box = None;
        self.fly_box_epoch = self.fly_box_epoch.wrapping_add(1);
        if self.section != PrimarySection::Home || self.new_session_open {
            self.page_epoch = self.page_epoch.wrapping_add(1);
        }
        self.section = PrimarySection::Home;
        self.new_session_open = false;
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
                self.sync_store_lineage(&store, cx);
                store.update(cx, |store, _| { store.snapshot_pending = true; store.buffered_frames.clear(); });
                let _ = self.requests.unbounded_send(BridgeRequest::Select(seed));
                self.current = Some(store);
                self.sync_sidebar_for_side_panel(cx);
                cx.notify();
            }
            None => {
                let requests = self.requests.clone();
                let anchors = self.handoff_anchors.clone();
                let draft = self.workbench.drafts.get(&seed).cloned().unwrap_or_default();
                let siwc_ready = self.siwc_ready();
                let ds_ready = self.dsaccount_signed_in();
                let store = cx.new(|cx| SessionStore::new(seed.clone(), requests, draft, window, cx));
                store.update(cx, |store, _| {
                    store.handoff_anchors = anchors;
                    store.catalog = self.providers.clone();
                    store.siwc_ready = siwc_ready;
                    store.dsaccount_signed_in = ds_ready;
                    store.client_session_id = match &self.connection {
                        qaqh_gpui_wire::bridge::ConnectionState::Ready { client_session_id } => Some(client_session_id.clone()), _ => None,
                    };
                });
                if let Some(cwd) = self.sessions.iter().find(|row| row.id == seed)
                    .and_then(|row| row.cwd.as_deref())
                {
                    store.update(cx, |store, _| store.code_panel.cwd = Some(cwd.into()));
                }
                self.sync_store_lineage(&store, cx);
                let sub = cx.subscribe_in(
                    &store,
                    window,
                    |daemon, _, event: &qaqh_gpui_session::store::SessionNavEvent, window, cx| {
                        match event {
                            qaqh_gpui_session::store::SessionNavEvent::OpenSession {
                                session_id,
                                fly_from_session,
                            } => {
                                daemon.jump_with_fly(
                                    session_id.clone(),
                                    fly_from_session.clone(),
                                    window,
                                    cx,
                                );
                            }
                            qaqh_gpui_session::store::SessionNavEvent::SidePanelOpenChanged {
                                ..
                            } => {
                                daemon.sync_sidebar_for_side_panel(cx);
                            }
                            qaqh_gpui_session::store::SessionNavEvent::OpenAccess => {
                                daemon.navigate(PrimarySection::Access, window, cx);
                            }
                        }
                    },
                );
                self.session_subs.push((seed.clone(), sub));
                self.stores.push(store.clone());
                self.current = Some(store);
                if self.stores.len() > 4 {
                    let evicted = self.stores.remove(0);
                    let evicted_id = evicted.read(cx).session_id.clone();
                    self.session_subs.retain(|(sid, _)| sid != &evicted_id);
                }
                let _ = self.requests.unbounded_send(BridgeRequest::Select(seed));
                self.sync_sidebar_for_side_panel(cx);
                cx.notify();
            }
        }
    }

    pub fn persist(&mut self, cx: &gpui_kit::App) {
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

/// 侧栏/全局共用标题清洗:绝不允许空串、"(无标题)"或 "ses_*" ULID 上屏。
pub(crate) fn clean_session_title(
    row: &SessionRow,
    workspaces: &[WorkspaceEntry],
) -> gpui_kit::SharedString {
    let raw = row.title.trim();
    if !raw.is_empty() && raw != "(无标题)" && !raw.starts_with("ses_") {
        return gpui_kit::SharedString::from(raw.to_owned());
    }
    row.workspace_id
        .as_ref()
        .and_then(|wid| {
            workspaces
                .iter()
                .find(|entry| &entry.id == wid)
                .map(|entry| gpui_kit::SharedString::from(entry.title.clone()))
        })
        .or_else(|| {
            row.cwd.as_ref().and_then(|cwd| {
                let trimmed = cwd.trim_end_matches(['/', '\\']);
                let tail = trimmed.rsplit(['/', '\\']).next().unwrap_or(trimmed);
                (!tail.is_empty()).then(|| gpui_kit::SharedString::from(tail.to_owned()))
            })
        })
        .unwrap_or_else(|| gpui_kit::SharedString::from("未命名会话"))
}
