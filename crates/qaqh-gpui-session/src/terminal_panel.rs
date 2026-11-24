//! 后台执行终端面板(electron ExecSessionDrawer / ReadOnlyTerminal 的 GPUI 同位)。
//!
//! 数据面 = RingingV2 exec session(后端 PTY 捕获流,协议最高准则,零转换层):
//! - live:exec_output_tail 增量(有 slot 时更鲜)否则 ExecSessionView.output_preview
//!   尾窗,offset 口径 = preview_start_offset / next_offset / slot end_offset;
//! - 终态:权威 output 全量 rehydrate(electron finalSnapshot 同口径);
//! - 落后或交叉保留窗口 → reset 重建;单实例屏,切换会话即重建(抽屉纪律 3)。
//!
//! 渲染内核 = [`crate::terminal::TerminalEmulator`] (`alacritty_terminal`)
//! + [`crate::terminal_view::TerminalView`] 原生只读 GPU 画布(支持滚轮回滚与选区复制)。

use std::cmp::Reverse;

use gpui_kit::prelude::*;
use gpui_kit::{
    App, ClickEvent, Context, Div, Entity, FontWeight, Hsla, SharedString, Window, div, px,
};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{
    RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, RADIUS_STAGE, shadow_card, shadow_stage,
};
use qaqh_gpui_kit::motion::live_dot;
use qaqh_gpui_kit::settings::Typography;
use qaqh_gpui_kit::theme::{Palette, terminal_palette};
use qaqh_ringing_contract::fact::ExecSessionId;
use qaqh_ringing_contract::state::ExecSessionState;

use crate::store::SessionStore;
use crate::terminal_view::TerminalView;
use crate::timeline::content_text;

/// 终态同步标记:终态权威 output 只 rehydrate 一次,此后屏面静态。
const FINAL_END: u64 = u64::MAX;

pub struct ActiveTerminal {
    pub id: ExecSessionId,
    /// 已喂入解析器的捕获流字节 offset(next_offset 口径)。
    pub applied: u64,
    pub is_final: bool,
}

pub struct TerminalPanel {
    pub open: bool,
    pub selected: Option<ExecSessionId>,
    /// 单实例屏:同一时刻只解析当前选中会话(抽屉纪律 3)。
    pub active: Option<ActiveTerminal>,
    /// 同步标记:(id, 目标 offset, final) 全等 → 屏面静态,sync 免克隆窗口文本。
    synced: Option<(ExecSessionId, u64, bool)>,
    /// 只读虚拟终端实体(支持滚轮回滚与鼠标拖拽/Ctrl+C 复制)。
    pub view: Entity<TerminalView>,
}

impl TerminalPanel {
    pub fn new(cx: &mut Context<SessionStore>) -> Self {
        let view = cx.new(|cx| TerminalView::new_readonly("exec-terminal-view", cx.focus_handle()));
        Self {
            open: false,
            selected: None,
            active: None,
            synced: None,
            view,
        }
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
    store.terminal_panel.open = true;
    store.terminal_panel.selected =
        Some(exec_id.unwrap_or_else(|| default_pick(store).expect("exec card 打开时必有会话")));
    store.terminal_panel.active = None;
    store.terminal_panel.synced = None;
    cx.notify();
}

pub fn close(store: &mut SessionStore, cx: &mut Context<SessionStore>) {
    store.terminal_panel.open = false;
    store.terminal_panel.selected = None;
    store.terminal_panel.active = None;
    store.terminal_panel.synced = None;
    cx.notify();
}

pub fn select(store: &mut SessionStore, exec_id: ExecSessionId, cx: &mut Context<SessionStore>) {
    store.terminal_panel.selected = Some(exec_id);
    store.terminal_panel.active = None;
    store.terminal_panel.synced = None;
    cx.notify();
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
    if !store.terminal_panel.open {
        let had_active = store.terminal_panel.active.take().is_some();
        let had_synced = store.terminal_panel.synced.take().is_some();
        if had_active || had_synced {
            let view = store.terminal_panel.view.clone();
            view.update(cx, |view, _| view.reset_quiet());
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
        view.update(cx, |view, _| {
            view.reset_quiet();
            view.feed_quiet(window_text.as_bytes());
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
                view.update(cx, |view, _| view.feed_quiet(tail.as_bytes()));
                active.applied = target_end;
                active.is_final = is_final;
            }
            None => {
                // offset 与窗口不对齐(仅理论上可达):按落后重建。
                view.update(cx, |view, _| {
                    view.reset_quiet();
                    view.feed_quiet(window_text.as_bytes());
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
        ExecSessionState::Running => ("后台运行".into(), t.accent),
        ExecSessionState::Stopping { .. } => ("正在停止".into(), t.warn),
        ExecSessionState::Finished { outcome } => match outcome {
            qaqh_ringing_contract::state::ExecSessionOutcome::Exited { exit_code: 0 } => {
                ("已结束".into(), t.ok)
            }
            qaqh_ringing_contract::state::ExecSessionOutcome::Exited { exit_code } => {
                (format!("已退出 · {exit_code}"), t.bad)
            }
            qaqh_ringing_contract::state::ExecSessionOutcome::Cancelled { .. } => {
                ("已取消".into(), t.bad)
            }
            qaqh_ringing_contract::state::ExecSessionOutcome::TimedOut { .. } => {
                ("已超时".into(), t.warn)
            }
            qaqh_ringing_contract::state::ExecSessionOutcome::Interrupted { .. } => {
                ("已中断".into(), t.bad)
            }
        },
    }
}

/// 动作 pill(electron exec-drawer-action-btn 同位):btn 面 + hairline 边,
/// hover 换语义色描边;GPUI 无阴影/动画,层级用面层与色变表达。
fn action_pill(
    id: SharedString,
    label: &'static str,
    hover_color: Hsla,
    t: &Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(24.))
        .px_2()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(t.border)
        .bg(t.btn)
        .text_xs()
        .text_color(t.text)
        .cursor_pointer()
        .hover(move |style| style.border_color(hover_color).text_color(hover_color))
        .child(SharedString::from(label))
        .on_click(on_click)
}

fn session_view(
    store: &SessionStore,
    t: &Palette,
    _ty: &Typography,
    cx: &mut Context<SessionStore>,
) -> Option<Div> {
    if !store.terminal_panel.open {
        return None;
    }
    let t = *t;
    let panel = &store.terminal_panel;
    // 运行中排前,其余按开始时间倒序(抽屉 entries 同口径)。
    let mut order: Vec<usize> = (0..store.exec_sessions.len()).collect();
    order.sort_by_key(|&index| {
        let session = &store.exec_sessions[index];
        (usize::from(session.state.is_terminal()), Reverse(session.started_at_ms))
    });

    let mut body = div().flex().flex_col().min_h_0().flex_1();
    if order.is_empty() {
        body = body.child(
            div().p_3().text_xs().text_color(t.dim).child("当前会话没有后台执行会话。"),
        );
        return Some(body);
    }

    // 会话列表:圆角胶囊条目 + 呼吸光点。
    let entity = cx.entity();
    let list_rows = div()
        .id("exec-session-list")
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .max_h(px(164.))
        .overflow_y_scroll()
        .border_b_1()
        .border_color(t.border_subtle)
        .children(order.iter().map(|&index| {
            let session = &store.exec_sessions[index];
            let (label, color) = state_view(&session.state, &t);
            let active = matches!(session.state, ExecSessionState::Running);
            let selected = panel.selected.as_ref() == Some(&session.id);
            let id = session.id.clone();
            let row_entity = entity.clone();
            div()
                .id(SharedString::from(format!("exec-chip-{}", id.0)))
                .flex()
                .flex_col()
                .gap_0p5()
                .px_2p5()
                .py_1p5()
                .rounded(px(RADIUS_CONTROL))
                .cursor_pointer()
                .when(selected, |el| {
                    el.bg(t.card)
                        .border_1()
                        .border_color(t.border_subtle)
                        .shadow(shadow_card(&t))
                })
                .when(!selected, |el| el.hover(|style| style.bg(t.hover)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .min_w_0()
                        .child(live_dot(format!("exec-{}", id.0), color, active))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .font_family("monospace")
                                .text_xs()
                                .text_color(t.text)
                                .child(SharedString::from(session.command.clone())),
                        ),
                )
                .child(
                    div()
                        .pl_4()
                        .text_xs()
                        .text_color(color)
                        .child(SharedString::from(label)),
                )
                .on_click(move |_, _, cx| {
                    let _ = row_entity.update(cx, |store, cx| {
                        if store.terminal_panel.selected.as_ref() != Some(&id) {
                            select(store, id.clone(), cx);
                        }
                    });
                })
        }));

    let selected = panel
        .selected
        .as_ref()
        .and_then(|id| store.exec_sessions.iter().find(|session| &session.id == id));
    body = body.child(list_rows).children(selected.map(|session| {
        let (label, color) = state_view(&session.state, &t);
        let id = session.id.clone();
        let copy_id = id.clone();
        let running = matches!(session.state, ExecSessionState::Running);
        let stopping = matches!(session.state, ExecSessionState::Stopping { .. });
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(t.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family("monospace")
                            .text_xs()
                            .text_color(t.text)
                            .child(SharedString::from(session.command.clone())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_xs()
                            .text_color(color)
                            .child(SharedString::from(label)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .flex_shrink_0()
                    .child(action_pill(
                        SharedString::from(format!("exec-copy-{}", id.0)),
                        "复制输出",
                        t.accent,
                        &t,
                        cx.listener(move |store, _, _, cx| {
                            let text = copy_text(store, &copy_id, cx);
                            if !text.is_empty() {
                                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(text));
                            }
                            cx.notify();
                        }),
                    ))
                    .children(running.then(|| {
                        let row_entity = entity.clone();
                        let stop_id = id.clone();
                        action_pill(
                            SharedString::from(format!("exec-int-{}", stop_id.0)),
                            "中断",
                            t.warn,
                            &t,
                            move |_, _, cx| {
                                let _ = row_entity.update(cx, |store, _| {
                                    store.exec_control(stop_id.0.clone(), false)
                                });
                            },
                        )
                    }))
                    .children((running || stopping).then(|| {
                        let row_entity = entity.clone();
                        let term_id = id.clone();
                        action_pill(
                            SharedString::from(format!("exec-term-{}", term_id.0)),
                            "终止",
                            t.bad,
                            &t,
                            move |_, _, cx| {
                                let _ = row_entity.update(cx, |store, _| {
                                    store.exec_control(term_id.0.clone(), true)
                                });
                            },
                        )
                    })),
            )
    }));

    // 终端区:原生 GPU 虚拟终端画布(占满剩余空间,绝不零高折叠)。
    let has_output = panel.active.is_some() && panel.view.read(cx).has_output();
    if !has_output {
        body = body.child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(t.dim)
                .child("选中会话尚无输出。"),
        );
        return Some(body);
    }
    let term = terminal_palette(
        qaqh_gpui_kit::settings::get(cx).appearance.terminal_scheme,
        &t,
    );
    body = body.child(
        div()
            .id("terminal-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .m_2p5()
            .rounded(px(RADIUS_CARD))
            .bg(term.bg)
            .border_1()
            .border_color(t.border_subtle)
            .p_2()
            .overflow_hidden()
            .child(panel.view.clone()),
    );
    Some(body)
}

/// 面板外框:匹配主舞台的悬浮圆角岛。
pub fn render(
    store: &SessionStore,
    t: &Palette,
    ty: &Typography,
    cx: &mut Context<SessionStore>,
) -> Option<Div> {
    if !store.terminal_panel.open {
        return None;
    }
    let running_count = store
        .exec_sessions
        .iter()
        .filter(|session| !session.state.is_terminal())
        .count();
    Some(
        div()
            .ml_2()
            .w(px(440.))
            .max_w(gpui_kit::relative(0.50))
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
            .child(
                div()
                    .h(px(42.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3p5()
                    .bg(t.surface_subtle)
                    .border_b_1()
                    .border_color(t.border_subtle)
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
                                qaqh_gpui_kit::kit::pill(
                                    &format!("{running_count} 运行中"),
                                    t.accent,
                                    t,
                                )
                            })),
                    )
                    .child(
                        div()
                            .id("close-terminal-panel")
                            .size(px(24.))
                            .rounded(px(RADIUS_CHIP))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|style| style.bg(t.hover))
                            .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.dim))
                            .on_click(cx.listener(|store, _, _, cx| close(store, cx))),
                    ),
            )
            .child(session_view(store, t, ty, cx).expect("open=true 时必有面板体")),
    )
}
