//! 后台执行观察窗(electron ExecSessionDrawer / ReadOnlyTerminal 的 GPUI 演进版)。
//!
//! 数据面 = RingingV2 exec session(后端 PTY 捕获流,协议最高准则,零转换层):
//! - live:exec_output_tail 增量(有 slot 时更鲜)否则 ExecSessionView.output_preview
//!   尾窗,offset 口径 = preview_start_offset / next_offset / slot end_offset;
//! - 终态:权威 output 全量 rehydrate(electron finalSnapshot 同口径);
//! - 落后或交叉保留窗口 → reset 重建;单实例屏,切换会话即重建(抽屉纪律 3)。
//!
//! 观察窗布局与宽对齐:
//! - 列宽锁定后端 exec PTY 列数(`BACKEND_EXEC_PTY_COLS = 160`),杜绝窄栏二次折行与光标定位错位;
//! - 默认 `10px` 高密度等宽字号(支持 `9–14px` 局部步进微调并持久化);
//! - 支持一键展开为居中 L3 悬浮大窗(`Option A`,复用 `Presence` 物理进退场 + `Esc` 还原)。

use std::cell::Cell;
use std::cmp::Reverse;

use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Div, Entity, FontWeight, Hsla,
    MouseButton, SharedString, Window, div, px, relative,
};
use qaqh_gpui_kit::kit::{
    RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, RADIUS_STAGE, shadow_card, shadow_popover,
    shadow_stage, soft,
};
use qaqh_gpui_kit::motion::{
    live_dot, sample_split_drawer, spring_f32, spring_panel_policy, wrap_split_drawer,
};
use qaqh_gpui_kit::settings::{self, Typography};
use qaqh_gpui_kit::theme::{Palette, terminal_palette};
use qaqh_ringing_contract::fact::ExecSessionId;
use qaqh_ringing_contract::state::{ExecSessionOutcome, ExecSessionState, ExecSessionView};

use crate::store::SessionStore;
use crate::terminal_view::{ColsMode, TerminalView};
use crate::timeline::content_text;

/// 后端 exec 会话当前固定列数(160 列);未来协议透传 `pty_cols` 时由此唯一入口接管。
pub const BACKEND_EXEC_PTY_COLS: usize = 160;
const EXEC_FONT_MIN: f32 = 9.0;
const EXEC_FONT_MAX: f32 = 14.0;

/// 终态同步标记:终态权威 output 只 rehydrate 一次,此后屏面静态。
const FINAL_END: u64 = u64::MAX;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PanelMode {
    Docked,
    Expanded,
}

impl PanelMode {
    fn id_prefix(self) -> &'static str {
        match self {
            Self::Docked => "exec-dock",
            Self::Expanded => "exec-modal",
        }
    }
}

pub struct ActiveTerminal {
    pub id: ExecSessionId,
    /// 已喂入解析器的捕获流字节 offset(next_offset 口径)。
    pub applied: u64,
    pub is_final: bool,
}

pub struct TerminalPanel {
    pub open: bool,
    pub expanded: bool,
    closing: bool,
    pub list_collapsed: bool,
    pub selected: Option<ExecSessionId>,
    /// 单实例屏:同一时刻只解析当前选中会话(抽屉纪律 3)。
    pub active: Option<ActiveTerminal>,
    /// 同步标记:(id, 目标 offset, final) 全等 → 屏面静态,sync 免克隆窗口文本。
    synced: Option<(ExecSessionId, u64, bool)>,
    synced_style: Option<(ColsMode, u32)>,
    /// 由 `DaemonStore` 在渲染期写入:指示 L3 展开浮窗是否正在挂载 `view`(含退场期),严禁双挂载。
    modal_visible: Cell<bool>,
    last_modal_sig: Vec<(ExecSessionId, u8)>,
    /// 只读虚拟终端实体(支持滚轮回滚、横向平移与鼠标拖拽/Ctrl+C 复制)。
    pub view: Entity<TerminalView>,
}

impl TerminalPanel {
    pub fn new(cx: &mut Context<SessionStore>) -> Self {
        let view = cx.new(|cx| {
            let mut view = TerminalView::new_readonly("exec-terminal-view", cx.focus_handle());
            view.set_cols_mode(ColsMode::FixedCols(BACKEND_EXEC_PTY_COLS));
            view.set_font_size(Some(10.0));
            view
        });
        Self {
            open: false,
            expanded: false,
            closing: false,
            list_collapsed: false,
            selected: None,
            active: None,
            synced: None,
            synced_style: None,
            modal_visible: Cell::new(false),
            last_modal_sig: Vec::new(),
            view,
        }
    }

    pub fn is_expanded(&self) -> bool {
        self.open && self.expanded
    }

    pub fn modal_visible(&self) -> bool {
        self.modal_visible.get()
    }

    pub fn set_modal_visible(&self, visible: bool) {
        self.modal_visible.set(visible);
    }
}

fn backend_cols_for(_session: Option<&ExecSessionView>) -> usize {
    BACKEND_EXEC_PTY_COLS
}

fn state_code(state: &ExecSessionState) -> u8 {
    match state {
        ExecSessionState::Running => 0,
        ExecSessionState::Stopping { .. } => 1,
        ExecSessionState::Finished { .. } => 2,
    }
}

/// 默认选中:最新运行中,否则最新开始(electron entries 排序同口径)。
fn default_pick(store: &SessionStore) -> Option<ExecSessionId> {
    store
        .exec_sessions
        .iter()
        .min_by_key(|session| {
            (usize::from(session.state.is_terminal()), Reverse(session.started_at_ms))
        })
        .map(|session| session.id.clone())
}

/// 打开面板;指定 exec_id 则选中,否则走默认选中。
pub fn open(
    store: &mut SessionStore,
    exec_id: Option<ExecSessionId>,
    cx: &mut Context<SessionStore>,
) {
    let was_open = store.terminal_panel.open;
    store.terminal_panel.open = true;
    store.terminal_panel.closing = false;
    store.terminal_panel.selected = exec_id.or_else(|| default_pick(store));
    store.terminal_panel.active = None;
    store.terminal_panel.synced = None;
    store.terminal_panel.synced_style = None;
    if !was_open {
        cx.emit(crate::store::SessionNavEvent::SidePanelOpenChanged { open: true });
    }
    cx.notify();
}

pub fn close(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    let was_open = store.terminal_panel.open;
    store.terminal_panel.open = false;
    store.terminal_panel.expanded = false;
    store.terminal_panel.closing = was_open;
    if !was_open {
        store.terminal_panel.selected = None;
        store.terminal_panel.active = None;
        store.terminal_panel.synced = None;
        store.terminal_panel.synced_style = None;
    } else {
        cx.emit(crate::store::SessionNavEvent::SidePanelOpenChanged { open: false });
    }
    cx.notify();
}

pub fn set_expanded(store: &mut SessionStore, expanded: bool, cx: &mut Context<SessionStore>) {
    let next = expanded && store.terminal_panel.open;
    if store.terminal_panel.expanded != next {
        store.terminal_panel.expanded = next;
        cx.notify();
    }
}

pub fn toggle_expanded(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    if store.terminal_panel.open {
        store.terminal_panel.expanded = !store.terminal_panel.expanded;
        cx.notify();
    }
}

pub fn toggle_list(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    store.terminal_panel.list_collapsed = !store.terminal_panel.list_collapsed;
    cx.notify();
}

pub fn select(store: &mut SessionStore, exec_id: ExecSessionId, cx: &mut Context<SessionStore>) {
    store.terminal_panel.selected = Some(exec_id);
    store.terminal_panel.active = None;
    store.terminal_panel.synced = None;
    cx.notify();
}

fn adjust_font_size(delta: f32, cx: &mut App) {
    let cur = settings::get(cx).typography.exec_font_size;
    let next = (cur + delta).round().clamp(EXEC_FONT_MIN, EXEC_FONT_MAX);
    if (next - cur).abs() > 0.01 {
        settings::update(cx, |s| s.typography.exec_font_size = next);
    }
}

/// 复制正文(优先复制当前框选内容,否则终态 output / 窗口文本)。
fn copy_text(store: &SessionStore, id: &ExecSessionId, cx: &App) -> String {
    if let Some(selected) = store.terminal_panel.view.read(cx).selection_text() {
        return selected;
    }
    let Some(session) = store.exec_sessions.iter().find(|session| &session.id == id) else {
        return String::new();
    };
    if let Some(text) = session.output.as_ref().filter(|_| session.state.is_terminal()) {
        return content_text(text).to_owned();
    }
    match store.exec_output_tail.iter().find(|(slot, _, _)| slot == id) {
        Some((_, _, text)) => text.clone(),
        None => session.output_preview.clone(),
    }
}

/// 面板打开时对齐解析态。SessionStore::render 顶部调用。
pub fn sync(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    let modal_visible = store.terminal_panel.modal_visible();
    if modal_visible {
        let sig: Vec<(ExecSessionId, u8)> = store
            .exec_sessions
            .iter()
            .map(|session| (session.id.clone(), state_code(&session.state)))
            .collect();
        if store.terminal_panel.last_modal_sig != sig {
            store.terminal_panel.last_modal_sig = sig;
            cx.defer(|cx| cx.refresh_windows());
        }
    } else if !store.terminal_panel.last_modal_sig.is_empty() {
        store.terminal_panel.last_modal_sig.clear();
    }

    if !store.terminal_panel.open {
        if !store.terminal_panel.closing {
            let had_active = store.terminal_panel.active.take().is_some();
            let had_synced = store.terminal_panel.synced.take().is_some();
            store.terminal_panel.synced_style = None;
            if had_active || had_synced {
                let view = store.terminal_panel.view.clone();
                view.update(cx, |view, _| view.reset_quiet());
            }
        }
        return;
    }
    let selected = store
        .terminal_panel
        .selected
        .clone()
        .or_else(|| default_pick(store));
    store.terminal_panel.selected = selected.clone();
    let Some(id) = selected else {
        store.terminal_panel.active = None;
        store.terminal_panel.synced = None;
        let view = store.terminal_panel.view.clone();
        view.update(cx, |view, _| view.reset_quiet());
        return;
    };
    let Some(session) = store.exec_sessions.iter().find(|session| session.id == id) else {
        // 会话消失(裁剪/清除):屏面一并释放,等下一次选中重建。
        store.terminal_panel.active = None;
        store.terminal_panel.synced = None;
        let view = store.terminal_panel.view.clone();
        view.update(cx, |view, _| view.reset_quiet());
        return;
    };

    let exec_font_size = settings::get(cx)
        .typography
        .exec_font_size
        .clamp(EXEC_FONT_MIN, EXEC_FONT_MAX);
    let cols_mode = if session.tty || !store.terminal_panel.expanded {
        ColsMode::FixedCols(backend_cols_for(Some(session)))
    } else {
        ColsMode::AutoFit
    };
    let style_key = (cols_mode, exec_font_size.to_bits());
    if store.terminal_panel.synced_style != Some(style_key) {
        store.terminal_panel.synced_style = Some(style_key);
        let view = store.terminal_panel.view.clone();
        view.update(cx, |view, vcx| {
            view.set_font_size(Some(exec_font_size));
            view.set_cols_mode(cols_mode);
            if modal_visible {
                vcx.notify();
            }
        });
    }

    let is_final = session.state.is_terminal() && session.output.is_some();
    let (target_end, window_start, window_text) = if is_final {
        (
            FINAL_END,
            0,
            session
                .output
                .as_ref()
                .map(content_text)
                .expect("is_final 已验证")
                .to_owned(),
        )
    } else if let Some((_, end, text)) = store
        .exec_output_tail
        .iter()
        .find(|(slot, _, _)| slot == &id)
    {
        (*end, end.saturating_sub(text.len() as u64), text.clone())
    } else {
        (
            session.next_offset,
            session.preview_start_offset,
            session.output_preview.clone(),
        )
    };

    if store.terminal_panel.synced.as_ref() == Some(&(id.clone(), target_end, is_final)) {
        return; // 屏面静态:数据未变,不克隆不重喂。
    }

    let behind = store.terminal_panel.active.as_ref().is_none_or(|active| {
        active.id != id
            || active.is_final != is_final
            || active.applied < window_start
            || active.applied > target_end
    });
    let view = store.terminal_panel.view.clone();
    if behind {
        view.update(cx, |view, vcx| {
            view.reset_quiet();
            view.feed_quiet(window_text.as_bytes());
            if modal_visible {
                vcx.notify();
            }
        });
        store.terminal_panel.active = Some(ActiveTerminal {
            id: id.clone(),
            applied: target_end,
            is_final,
        });
    } else if let Some(active) = store.terminal_panel.active.as_mut() {
        // 窗口内增量推进;skip 落在 String 字节边界(此前整段喂过的前缀)。
        let skip = (active.applied - window_start) as usize;
        match window_text.get(skip..) {
            Some(tail) => {
                view.update(cx, |view, vcx| {
                    view.feed_quiet(tail.as_bytes());
                    if modal_visible {
                        vcx.notify();
                    }
                });
                active.applied = target_end;
                active.is_final = is_final;
            }
            None => {
                // offset 与窗口不对齐(仅理论上可达):按落后重建。
                view.update(cx, |view, vcx| {
                    view.reset_quiet();
                    view.feed_quiet(window_text.as_bytes());
                    if modal_visible {
                        vcx.notify();
                    }
                });
                active.applied = target_end;
                active.is_final = is_final;
            }
        }
    }
    store.terminal_panel.synced = Some((id, target_end, is_final));
}

/// 状态标签与着色(electron execStateLabel / execStateClass 同口径)。
fn state_view(state: &ExecSessionState, t: &Palette) -> (String, Hsla) {
    match state {
        ExecSessionState::Running => ("运行中".into(), t.accent),
        ExecSessionState::Stopping { .. } => ("停止中".into(), t.warn),
        ExecSessionState::Finished { outcome } => match outcome {
            ExecSessionOutcome::Exited { exit_code: 0 } => ("已结束".into(), t.ok),
            ExecSessionOutcome::Exited { exit_code } => (format!("退出 {exit_code}"), t.bad),
            ExecSessionOutcome::Cancelled { .. } => ("已取消".into(), t.bad),
            ExecSessionOutcome::TimedOut { .. } => ("已超时".into(), t.warn),
            ExecSessionOutcome::Interrupted { .. } => ("已中断".into(), t.bad),
        },
    }
}

/// icon-first 紧凑工具按钮(DESIGN-LAW §20:持久控件一律 Lucide Icon + tooltip)。
fn icon_btn(
    id: SharedString,
    icon: IconName,
    color: Hsla,
    hover_color: Hsla,
    tooltip: impl Into<SharedString>,
    t: &Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    let tooltip: SharedString = tooltip.into();
    let hover_bg = t.hover;
    div()
        .id(id)
        .size(px(24.))
        .rounded(px(RADIUS_CHIP))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(color)
        .hover(move |style| style.bg(hover_bg).text_color(hover_color))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .on_click(on_click)
        .child(Icon::new(icon).with_size(Size::XSmall))
}

fn sorted_order(store: &SessionStore) -> Vec<usize> {
    let mut order: Vec<usize> = (0..store.exec_sessions.len()).collect();
    order.sort_by_key(|&index| {
        let session = &store.exec_sessions[index];
        (usize::from(session.state.is_terminal()), Reverse(session.started_at_ms))
    });
    order
}

fn session_tooltip(session: &ExecSessionView) -> SharedString {
    match session.cwd.as_deref().filter(|cwd| !cwd.is_empty()) {
        Some(cwd) => SharedString::from(format!("{}\ncwd: {cwd}", session.command)),
        None => SharedString::from(session.command.clone()),
    }
}

/// 单行紧凑会话条目(严格 ≤28px,对齐 DESIGN-LAW 密度预算)。
fn session_row(
    session: &ExecSessionView,
    selected: bool,
    prefix: &str,
    entity: Entity<SessionStore>,
    t: &Palette,
) -> gpui_kit::Stateful<Div> {
    let (label, color) = state_view(&session.state, t);
    let active = matches!(session.state, ExecSessionState::Running);
    let id = session.id.clone();
    let tip = session_tooltip(session);
    let selected_bg = t.selected;
    let hover_bg = t.hover;

    div()
        .id(SharedString::from(format!("{prefix}-row-{}", id.0)))
        .h(px(28.))
        .px_2()
        .rounded(px(RADIUS_CONTROL))
        .flex()
        .items_center()
        .gap_1p5()
        .cursor_pointer()
        .when(selected, |el| el.bg(selected_bg))
        .when(!selected, |el| el.hover(move |style| style.bg(hover_bg)))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .child(live_dot(format!("{prefix}-dot-{}", id.0), color, active))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_xs()
                .text_color(if selected { t.text } else { t.dim })
                .child(SharedString::from(session.command.clone())),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_size(px(10.))
                .text_color(color)
                .child(SharedString::from(label)),
        )
        .on_click(move |_, window, cx| {
            let _ = entity.update(cx, |store, cx| {
                if store.terminal_panel.selected.as_ref() != Some(&id) {
                    select(store, id.clone(), cx);
                }
            });
            window.refresh();
        })
}

fn render_header(
    store: &SessionStore,
    entity: Entity<SessionStore>,
    session_count: usize,
    running_count: usize,
    selected_session: Option<&ExecSessionView>,
    mode: PanelMode,
    t: &Palette,
    cx: &App,
) -> gpui_kit::Stateful<Div> {
    let prefix = mode.id_prefix();
    let font_size = settings::get(cx)
        .typography
        .exec_font_size
        .clamp(EXEC_FONT_MIN, EXEC_FONT_MAX);
    let cols = backend_cols_for(selected_session);
    let meta_label = SharedString::from(format!("{cols}c · {}px", font_size.round() as i32));
    let meta_tip = SharedString::from(format!(
        "后端 PTY 列对齐: {cols} 列 · 字号 {}px\n支持触控板或横向滚轮平移",
        font_size.round() as i32
    ));

    let shrink_entity = entity.clone();
    let grow_entity = entity.clone();
    let expand_entity = entity.clone();
    let list_entity = entity.clone();
    let close_entity = entity.clone();
    let dbl_entity = entity;

    div()
        .id(SharedString::from(format!("{prefix}-header")))
        .h(px(36.))
        .px_3()
        .bg(t.surface_subtle)
        .border_b_1()
        .border_color(t.border_subtle)
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            if event.click_count >= 2 {
                let _ = dbl_entity.update(cx, |store, cx| toggle_expanded(store, cx));
                window.refresh();
                cx.stop_propagation();
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(
                    Icon::new(IconName::SquareTerminal)
                        .with_size(Size::XSmall)
                        .text_color(t.accent),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child("后台执行"),
                )
                .children((running_count > 0).then(|| {
                    div()
                        .text_xs()
                        .text_color(t.dim)
                        .child(SharedString::from(format!("{running_count} 运行中")))
                }))
                .child(
                    div()
                        .id(SharedString::from(format!("{prefix}-cols-meta")))
                        .text_size(px(10.))
                        .text_color(t.faint)
                        .tooltip(move |window, cx| {
                            Tooltip::new(meta_tip.clone()).build(window, cx)
                        })
                        .child(meta_label),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_0p5()
                .flex_shrink_0()
                .child(icon_btn(
                    SharedString::from(format!("{prefix}-font-dec")),
                    IconName::Minus,
                    t.dim,
                    t.text,
                    "缩小观察窗终端字号 (最小 9px)",
                    t,
                    move |_, window, cx| {
                        adjust_font_size(-1.0, cx);
                        let _ = shrink_entity.update(cx, |_, cx| cx.notify());
                        window.refresh();
                    },
                ))
                .child(icon_btn(
                    SharedString::from(format!("{prefix}-font-inc")),
                    IconName::Plus,
                    t.dim,
                    t.text,
                    "放大观察窗终端字号 (最大 14px)",
                    t,
                    move |_, window, cx| {
                        adjust_font_size(1.0, cx);
                        let _ = grow_entity.update(cx, |_, cx| cx.notify());
                        window.refresh();
                    },
                ))
                .children((mode == PanelMode::Docked && session_count > 1).then(|| {
                    let collapsed = store.terminal_panel.list_collapsed;
                    icon_btn(
                        SharedString::from(format!("{prefix}-toggle-list")),
                        if collapsed {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronUp
                        },
                        t.dim,
                        t.text,
                        if collapsed { "展开会话列表" } else { "折叠会话列表" },
                        t,
                        move |_, window, cx| {
                            let _ = list_entity.update(cx, |store, cx| toggle_list(store, cx));
                            window.refresh();
                        },
                    )
                }))
                .child(icon_btn(
                    SharedString::from(format!("{prefix}-toggle-expand")),
                    if mode == PanelMode::Expanded {
                        IconName::Minimize2
                    } else {
                        IconName::Maximize2
                    },
                    t.dim,
                    t.text,
                    if mode == PanelMode::Expanded {
                        "还原侧栏观察窗"
                    } else {
                        "展开宽屏观察窗"
                    },
                    t,
                    move |_, window, cx| {
                        let _ = expand_entity.update(cx, |store, cx| toggle_expanded(store, cx));
                        window.refresh();
                    },
                ))
                .child(icon_btn(
                    SharedString::from(format!("{prefix}-close")),
                    IconName::X,
                    t.dim,
                    t.text,
                    "关闭观察窗",
                    t,
                    move |_, window, cx| {
                        let _ = close_entity.update(cx, |store, cx| close(store, cx));
                        window.refresh();
                    },
                )),
        )
}

fn render_info_bar(
    session: &ExecSessionView,
    entity: Entity<SessionStore>,
    prefix: &str,
    t: &Palette,
) -> Div {
    let (label, color) = state_view(&session.state, t);
    let id = session.id.clone();
    let copy_id = id.clone();
    let running = matches!(session.state, ExecSessionState::Running);
    let stopping = matches!(session.state, ExecSessionState::Stopping { .. });
    let tip = session_tooltip(session);

    let copy_entity = entity.clone();
    let int_entity = entity.clone();
    let term_entity = entity;

    div()
        .h(px(32.))
        .px_3()
        .border_b_1()
        .border_color(t.border_subtle)
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .id(SharedString::from(format!("{prefix}-cmd-title")))
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                        .text_xs()
                        .text_color(t.text)
                        .child(SharedString::from(session.command.clone())),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(10.))
                        .text_color(color)
                        .child(SharedString::from(label)),
                ),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_0p5()
                .flex_shrink_0()
                .child(icon_btn(
                    SharedString::from(format!("{prefix}-copy-{}", id.0)),
                    IconName::Copy,
                    t.dim,
                    t.accent,
                    "复制选区或全部输出",
                    t,
                    move |_, _, cx| {
                        let text = {
                            let store = copy_entity.read(cx);
                            copy_text(store, &copy_id, cx)
                        };
                        if !text.is_empty() {
                            cx.write_to_clipboard(ClipboardItem::new_string(text));
                        }
                    },
                ))
                .children(running.then(|| {
                    let stop_id = id.clone();
                    icon_btn(
                        SharedString::from(format!("{prefix}-int-{}", stop_id.0)),
                        IconName::CircleStop,
                        t.dim,
                        t.warn,
                        "中断进程 (SIGINT)",
                        t,
                        move |_, window, cx| {
                            let _ = int_entity.update(cx, |store, _| {
                                store.exec_control(stop_id.0.clone(), false);
                            });
                            window.refresh();
                        },
                    )
                }))
                .children((running || stopping).then(|| {
                    let term_id = id.clone();
                    icon_btn(
                        SharedString::from(format!("{prefix}-term-{}", term_id.0)),
                        IconName::OctagonX,
                        t.dim,
                        t.bad,
                        "强制终止进程",
                        t,
                        move |_, window, cx| {
                            let _ = term_entity.update(cx, |store, _| {
                                store.exec_control(term_id.0.clone(), true);
                            });
                            window.refresh();
                        },
                    )
                })),
        )
}

fn render_canvas_box(store: &SessionStore, prefix: &str, t: &Palette, cx: &App) -> AnyElement {
    let panel = &store.terminal_panel;
    let has_output = panel.active.is_some() && panel.view.read(cx).has_output();
    if !has_output {
        return div()
            .flex_1()
            .min_h_0()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .text_color(t.dim)
            .child("选中会话尚无输出。")
            .into_any_element();
    }

    let term = terminal_palette(settings::get(cx).appearance.terminal_scheme, t);
    let scrolled_up = panel.view.read(cx).display_offset() > 0;
    let view = panel.view.clone();
    let card_shadow = shadow_card(t);
    let hover_bg = t.hover;
    let tip = SharedString::from("回到底部跟随实时输出");

    div()
        .id(SharedString::from(format!("{prefix}-terminal-body")))
        .relative()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .mx_2()
        .my_2()
        .rounded(px(RADIUS_CARD))
        .bg(term.bg)
        .border_1()
        .border_color(t.border_subtle)
        .p_2()
        .overflow_hidden()
        .child(panel.view.clone())
        .children(scrolled_up.then(|| {
            div()
                .id(SharedString::from(format!("{prefix}-scroll-bottom")))
                .absolute()
                .right_3()
                .bottom_3()
                .size(px(28.))
                .rounded_full()
                .bg(t.card)
                .border_1()
                .border_color(t.border)
                .shadow(card_shadow)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(t.dim)
                .hover(move |style| style.bg(hover_bg).text_color(t.text))
                .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(move |_, window, cx| {
                    view.update(cx, |v, cx| v.scroll_to_bottom(cx));
                    window.refresh();
                })
                .child(Icon::new(IconName::ArrowDownToLine).with_size(Size::XSmall))
        }))
        .into_any_element()
}

fn render_body(
    store: &SessionStore,
    entity: Entity<SessionStore>,
    order: &[usize],
    selected_session: Option<&ExecSessionView>,
    mode: PanelMode,
    t: &Palette,
    cx: &App,
) -> Div {
    let prefix = mode.id_prefix();
    if order.is_empty() {
        return div().flex().flex_col().min_h_0().flex_1().child(
            div()
                .p_3()
                .text_xs()
                .text_color(t.dim)
                .child("当前会话没有后台执行会话。"),
        );
    }

    let panel = &store.terminal_panel;
    let render_rows = |list_div: gpui_kit::Stateful<Div>| {
        list_div.children(order.iter().map(|&index| {
            let session = &store.exec_sessions[index];
            let selected = panel.selected.as_ref() == Some(&session.id);
            session_row(session, selected, prefix, entity.clone(), t)
        }))
    };

    let detail_pane = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .children(selected_session.map(|session| render_info_bar(session, entity.clone(), prefix, t)))
        .child(render_canvas_box(store, prefix, t, cx));

    match mode {
        PanelMode::Docked => {
            let show_list = order.len() > 1 && !panel.list_collapsed;
            let list = show_list.then(|| {
                render_rows(
                    div()
                        .id("exec-dock-session-list")
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .p_1p5()
                        .max_h(px(96.))
                        .overflow_y_scroll()
                        .border_b_1()
                        .border_color(t.border_subtle),
                )
            });
            div()
                .flex()
                .flex_col()
                .min_h_0()
                .flex_1()
                .children(list)
                .child(detail_pane)
        }
        PanelMode::Expanded => {
            let show_rail = order.len() > 1;
            let rail = show_rail.then(|| {
                render_rows(
                    div()
                        .id("exec-modal-session-list")
                        .w(px(240.))
                        .h_full()
                        .flex_shrink_0()
                        .bg(t.surface_subtle)
                        .border_r_1()
                        .border_color(t.border_subtle)
                        .p_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .overflow_y_scroll(),
                )
            });
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .children(rail)
                .child(detail_pane)
        }
    }
}

/// 侧栏观察窗外框:与代码面板共用二分抽屉物理弹簧(`sample_split_drawer` + `wrap_split_drawer`),
/// 展开时自动收回左侧工作区列表,与消息区构成左右二分。
pub fn render(
    store: &mut SessionStore,
    t: &Palette,
    _ty: &Typography,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
) -> Option<Div> {
    if store.terminal_panel.modal_visible() {
        let _ = spring_f32(
            "exec-panel-width",
            0.0,
            spring_panel_policy().with_travel(false),
            window,
            cx,
        );
        return None;
    }

    let sibling_open = store.code_panel.is_open();
    let Some(sample) = sample_split_drawer(
        "exec-panel-width",
        store.terminal_panel.open,
        sibling_open,
        window,
        cx,
    ) else {
        if store.terminal_panel.closing {
            store.terminal_panel.closing = false;
            store.terminal_panel.selected = None;
            store.terminal_panel.active = None;
            store.terminal_panel.synced = None;
            store.terminal_panel.synced_style = None;
            let view = store.terminal_panel.view.clone();
            view.update(cx, |view, _| view.reset_quiet());
        }
        return None;
    };

    let order = sorted_order(store);
    let running_count = store
        .exec_sessions
        .iter()
        .filter(|session| !session.state.is_terminal())
        .count();
    let selected_session = store
        .terminal_panel
        .selected
        .as_ref()
        .and_then(|id| store.exec_sessions.iter().find(|session| &session.id == id));

    let entity = cx.entity();
    let card = div()
        .w(px(sample.card_width))
        .h_full()
        .min_h_0()
        .flex()
        .flex_col()
        .rounded(px(RADIUS_STAGE))
        .bg(t.stage)
        .border_1()
        .border_color(t.border_subtle)
        .shadow(shadow_stage(t))
        .overflow_hidden()
        .child(render_header(
            store,
            entity.clone(),
            order.len(),
            running_count,
            selected_session,
            PanelMode::Docked,
            t,
            cx,
        ))
        .child(render_body(
            store,
            entity,
            &order,
            selected_session,
            PanelMode::Docked,
            t,
            cx,
        ));

    Some(wrap_split_drawer(sample, card.into_any_element(), cx))
}

/// Option A 居中 L3 悬浮观察大窗(由 `DaemonStore` 经 `presence_overlay` 驱动进退场)。
pub fn render_modal(
    session: &Entity<SessionStore>,
    progress: f32,
    interactive_backdrop: bool,
    t: &Palette,
    _window: &mut Window,
    cx: &mut App,
) -> Option<AnyElement> {
    let store = session.read(cx);
    if !store.terminal_panel.open {
        return None;
    }

    let order = sorted_order(store);
    let running_count = store
        .exec_sessions
        .iter()
        .filter(|item| !item.state.is_terminal())
        .count();
    let selected_session = store
        .terminal_panel
        .selected
        .as_ref()
        .and_then(|id| store.exec_sessions.iter().find(|item| &item.id == id));

    let p = progress.clamp(0.0, 1.0);
    let backdrop_entity = session.clone();
    let card = div()
        .id("exec-modal-card")
        .w(relative(0.92))
        .max_w(px(1280.))
        .h(relative(0.88))
        .max_h(px(860.))
        .mt(px((1.0 - p) * 14.0))
        .opacity(p)
        .rounded(px(RADIUS_STAGE))
        .bg(t.stage)
        .border_1()
        .border_color(t.border)
        .shadow(shadow_popover(t))
        .overflow_hidden()
        .flex()
        .flex_col()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, _, cx| cx.stop_propagation())
        .child(render_header(
            store,
            session.clone(),
            order.len(),
            running_count,
            selected_session,
            PanelMode::Expanded,
            t,
            cx,
        ))
        .child(render_body(
            store,
            session.clone(),
            &order,
            selected_session,
            PanelMode::Expanded,
            t,
            cx,
        ));

    Some(
        div()
            .id("exec-modal-backdrop")
            .absolute()
            .inset_0()
            .when(interactive_backdrop, |el| el.occlude())
            .bg(soft(t.scrim, t.scrim.a * p))
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                let _ = backdrop_entity.update(cx, |store, cx| set_expanded(store, false, cx));
                window.refresh();
            })
            .child(card)
            .into_any_element(),
    )
}
