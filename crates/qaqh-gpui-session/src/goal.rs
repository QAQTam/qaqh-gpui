//! Goal 模式 UI(ADR-0015):状态徽标 + 详情/控制 Dialog。
//! 语义纪律:取消续跑回合 ≠ 停止目标(要停必须显式 pause);blocked/complete
//! 是审计声明,用户只控制 active/paused;用量是内核记账权威值,前端不自算。

use gpui_kit::prelude::*;
use gpui_kit::{Context, Stateful, Div, Hsla, SharedString, Window, div, px};
use gpui_kit::component::dialog::Dialog;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::component::WindowExt;
use gpui_kit::assets::IconName;
use qaqh_ringing_contract::state::GoalStatus;
use qaqh_ringing_contract::state::GoalView;

use crate::store::SessionStore;
use qaqh_gpui_kit::theme::ActiveTheme as _;
use qaqh_gpui_kit::theme::Palette;

/// 状态元数据:(标签, 颜色)。用户可控态 = active/paused;其余按语义着色
/// 提示"需要人看一眼"(blocked)或系统熔断(usage/budget_limited)。
pub fn status_meta(status: GoalStatus, t: &Palette) -> (&'static str, Hsla) {
    match status {
        GoalStatus::Active => ("目标 · 进行中", t.ok),
        GoalStatus::Paused => ("目标 · 已暂停", t.warn),
        GoalStatus::Blocked => ("目标 · 已受阻", t.bad),
        GoalStatus::UsageLimited => ("目标 · 用量熔断", t.warn),
        GoalStatus::BudgetLimited => ("目标 · 预算熔断", t.warn),
        GoalStatus::Complete => ("目标 · 已完成", t.ok),
    }
}

/// 头部徽标:圆角状态胶囊(供 TitleBar 面包屑调用);点击打开详情 Dialog。
pub fn goal_badge(
    goal: &GoalView,
    session: &gpui_kit::Entity<SessionStore>,
    t: &Palette,
) -> Stateful<Div> {
    let (label, color) = status_meta(goal.status, t);
    let objective: String = goal.objective.chars().take(20).collect();
    let entity = session.clone();
    div()
        .id("goal-badge")
        .h(px(24.))
        .px_2p5()
        .rounded_full()
        .bg(qaqh_gpui_kit::kit::soft(color, 0.12))
        .border_1()
        .border_color(qaqh_gpui_kit::kit::soft(color, 0.24))
        .text_xs()
        .cursor_pointer()
        .flex()
        .items_center()
        .gap_1p5()
        .hover(|style| style.bg(qaqh_gpui_kit::kit::soft(color, 0.2)))
        .on_mouse_down(gpui_kit::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window: &mut Window, cx: &mut gpui_kit::App| {
            let _ = entity.update(cx, |store, cx| open_dialog(store, window, cx));
        })
        .child(
            Icon::new(IconName::Target)
                .with_size(Size::XSmall)
                .text_color(color),
        )
        .child(SharedString::from(label.to_owned()))
        .when(!objective.is_empty(), |badge| {
            badge.child(
                div()
                    .text_color(qaqh_gpui_kit::kit::soft(color, 0.8))
                    .child(SharedString::from(objective)),
            )
        })
}

/// 打开 goal Dialog:控制条入口与徽标入口同源;草稿每次打开重建种当前值。
pub fn open_dialog(store: &mut SessionStore, window: &mut Window, cx: &mut Context<SessionStore>) {
    // 草稿种当前值,避免显示漂移;每次打开重建。
    let objective = store
        .goal
        .as_ref()
        .map(|goal| goal.objective.clone())
        .unwrap_or_default();
    let budget = store
        .goal
        .as_ref()
        .and_then(|goal| goal.token_budget)
        .map(|budget| budget.to_string())
        .unwrap_or_default();
    let objective_field = cx.new(|cx| {
        InputState::new(window, cx).placeholder("目标描述(留空 = 仅改状态/预算)")
    });
    objective_field.update(cx, |field, cx| field.set_value(&objective, window, cx));
    let budget_field = cx.new(|cx| {
        InputState::new(window, cx).placeholder("Token 预算(留空 = 不动,0 拒收)")
    });
    budget_field.update(cx, |field, cx| field.set_value(&budget, window, cx));
    store.goal_draft = Some((objective_field, budget_field));
    let entity = cx.entity();
    window.open_dialog(cx, move |dialog, window, cx| {
        entity.update(cx, |store, cx| goal_dialog(store, dialog, window, cx))
    });
}

/// goal 详情/控制 Dialog:用量面板 + objective/预算编辑 + 状态控制。
pub fn goal_dialog(
    store: &mut SessionStore,
    dialog: Dialog,
    _window: &mut Window,
    cx: &mut Context<SessionStore>,
) -> Dialog {
    let t = *cx.colors();
    let Some((objective_field, budget_field)) = store.goal_draft.clone() else {
        return dialog;
    };
    let mut body: Vec<gpui_kit::AnyElement> = Vec::new();
    if let Some(goal) = store.goal.clone() {
        let (label, color) = status_meta(goal.status, &t);
        body.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(qaqh_gpui_kit::kit::pill(label, color, &t))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.dim)
                        .child(SharedString::from(format!(
                            "已用 {} tok · {} 分钟",
                            goal.tokens_used,
                            goal.time_used_ms / 60_000,
                        ))),
                )
                .into_any_element(),
        );
        if let Some(budget) = goal.token_budget {
            let ratio = if budget == 0 {
                0.0
            } else {
                (goal.tokens_used as f32 / budget as f32).clamp(0., 1.)
            };
            let percent = (ratio * 100.) as u32;
            body.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(SharedString::from(format!(
                                "预算 {} / {} tok({percent}%)",
                                goal.tokens_used, budget,
                            ))),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w_full()
                            .flex()
                            .rounded_full()
                            .bg(t.hover)
                            .child(
                                div()
                                    .h_full()
                                    .flex_grow(ratio)
                                    .rounded_full()
                                    .bg(if ratio >= 1. { t.bad } else { color }),
                            ),
                    )
                    .into_any_element(),
            );
        }
    }
    body.push(Input::new(&objective_field).small().w_full().into_any_element());
    body.push(Input::new(&budget_field).small().w_full().into_any_element());

    // 状态控制:用户主权 = active/paused;complete/blocked 只读展示。
    let status = store.goal.as_ref().map(|goal| goal.status);
    let has_goal = store.goal.is_some();
    let mut footer = div().flex().justify_end().gap_2();
    if let Some(goal_status) = status {
        if goal_status != GoalStatus::Paused {
            footer = footer.child(gpui_kit::AnyElement::from(qaqh_gpui_kit::kit::button(
                cx,
                qaqh_gpui_kit::kit::BtnKind::Ghost,
                "goal-pause",
                "暂停",
                &t,
                cx.listener(|store, _, window: &mut Window, cx| {
                    store.goal_set(None, Some(GoalStatus::Paused), None);
                    window.close_dialog(cx);
                    cx.notify();
                }),
            )));
        }
        if goal_status == GoalStatus::Paused
            || matches!(goal_status, GoalStatus::Blocked | GoalStatus::UsageLimited | GoalStatus::BudgetLimited)
        {
            footer = footer.child(gpui_kit::AnyElement::from(qaqh_gpui_kit::kit::button(
                cx,
                qaqh_gpui_kit::kit::BtnKind::Soft,
                "goal-resume",
                "恢复",
                &t,
                cx.listener(|store, _, window: &mut Window, cx| {
                    store.goal_set(None, Some(GoalStatus::Active), None);
                    window.close_dialog(cx);
                    cx.notify();
                }),
            )));
        }
        footer = footer.child(gpui_kit::AnyElement::from(qaqh_gpui_kit::kit::button(
            cx,
            qaqh_gpui_kit::kit::BtnKind::Danger,
            "goal-clear",
            "清除目标",
            &t,
            cx.listener(|store, _, window: &mut Window, cx| {
                store.goal_clear();
                window.close_dialog(cx);
                cx.notify();
            }),
        )));
    }
    footer = footer.child(gpui_kit::AnyElement::from(qaqh_gpui_kit::kit::button(
        cx,
        if has_goal { qaqh_gpui_kit::kit::BtnKind::Ghost } else { qaqh_gpui_kit::kit::BtnKind::Primary },
        "goal-save",
        if has_goal { "保存修改" } else { "设定目标" },
        &t,
        cx.listener(|store, _, window: &mut Window, cx| {
            let objective = store
                .goal_draft
                .as_ref()
                .and_then(|(field, _)| {
                    Some(field.read(cx).value().trim().to_string()).filter(|text| !text.is_empty())
                });
            let budget = store.goal_draft.as_ref().and_then(|(_, field)| {
                field
                    .read(cx)
                    .value()
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .map(Some)
            });
            match (objective, budget) {
                (None, None) => {}
                (objective, budget) => store.goal_set(objective, None, budget),
            }
            window.close_dialog(cx);
            cx.notify();
        }),
    )));

    dialog
        .title("会话目标")
        .children(body)
        .footer(footer)
}
