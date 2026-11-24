//! 工作台视图(DESIGN LAW v3.0 · v5.0.0):
//! - 侧栏中性胶囊行(去冗余灰点);
//! - 右侧弹簧 Inspector(任务进度条 + 团队协作板合一);
//! - 居中悬浮模态设置工作台(分段滑块 + 风格/动效切换) + 新建会话面板。

use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, ClickEvent, Context, Div, FontWeight, SharedString,
    Window, div, px, relative,
};
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, Sizable, Size, WindowExt as _};
use gpui_kit::assets::IconName;
use qaqh_config_api::ConfigPatch;
use qaqh_gpui_kit::kit::{
    BtnKind, RADIUS_CHIP, RADIUS_CONTROL, button, pill, segmented_item, segmented_strip, soft,
    stage_surface,
};
use qaqh_gpui_kit::motion::{live_dot, spring_f32, spring_panel_policy};
use qaqh_gpui_kit::primitives::{
    nav_pill, section_card, segmented_button, setting_row, step_button,
};
use qaqh_gpui_kit::settings::{self, Density, MotionLevel, TerminalScheme};
use qaqh_gpui_kit::theme::{ActiveTheme, Palette, ThemeMode, ThemePreset};
use qaqh_gpui_session::store::{SessionRow, SettingsEditor};
use qaqh_gpui_wire::bridge::{BridgeRequest, ConnectionState};
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::state::{GoalStatus, TimelineEntry};
use qaqh_types::SessionRunStatus;

use crate::daemon::{DaemonStore, InspectorTab};

/// 侧栏条目:中性悬浮胶囊;仅运行中/等待授权/失败时显示状态点,历史空闲会话干净留白。
pub(crate) fn sidebar_row(
    row: &SessionRow,
    display_title: &str,
    selected: bool,
    tone: Option<bool>,
    t: &Palette,
    animate_motion: bool,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    let dot = match row.status {
        SessionRunStatus::Running => Some((t.accent, animate_motion)),
        SessionRunStatus::WaitingUser => Some((t.warn, animate_motion)),
        _ if tone == Some(false) => Some((t.bad, false)),
        _ => None,
    };
    div()
        .id(SharedString::from(format!("sess-{}", row.id)))
        .mx_0p5()
        .px_2p5()
        .py_1p5()
        .rounded(px(RADIUS_CONTROL))
        .cursor_pointer()
        .on_click(listener)
        .flex()
        .items_center()
        .gap_2()
        .when(selected, |style| style.bg(t.selected).text_color(t.text))
        .when(!selected, |style| {
            style
                .text_color(t.dim)
                .hover(|h| h.bg(t.hover).text_color(t.text))
        })
        .children(dot.map(|(color, pulse)| live_dot(format!("sb-{}", row.id), color, pulse)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.))
                .text_sm()
                .font_weight(if selected {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .truncate()
                .child(SharedString::from(display_title.to_owned())),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(t.faint)
                .child(qaqh_gpui_kit::kit::relative_time(row.updated_at)),
        )
}

fn format_tok_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

fn overview_tool_mode_label(mode: &str) -> &'static str {
    if mode == qaqh_types::MINIMAL {
        "精简"
    } else if mode == qaqh_types::MINIMAL_B {
        "精简 B"
    } else if mode == qaqh_types::MINIMAL_C {
        "精简 C"
    } else if mode == qaqh_types::CUSTOM {
        "自定义"
    } else {
        "标准"
    }
}

fn overview_stat_box(label: &'static str, value: String, t: &Palette) -> Div {
    div()
        .flex_1()
        .p_2p5()
        .rounded(px(RADIUS_CONTROL))
        .bg(t.surface_subtle)
        .border_1()
        .border_color(t.border_subtle)
        .flex()
        .flex_col()
        .gap_0p5()
        .child(div().text_xs().text_color(t.dim).child(label))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .font_family("monospace")
                .text_color(t.text)
                .child(value),
        )
}

/// 右侧弹簧 Inspector(会话概览 / 任务清单 / 团队协作板合一):
/// 激活任意会话即随时可展开概览;有任务或团队数据时自动显示切换标签,物理弹簧平滑收折。
pub(crate) fn inspector_dock(
    daemon: &mut DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
) -> Option<AnyElement> {
    let current_entity = daemon.current.clone()?;
    let t = *cx.colors();
    let current_seed = current_entity.read(cx).session_id.clone();

    let todo_items = match &daemon.todos {
        Some((seed, items)) if seed == &current_seed && !items.is_empty() => Some(items),
        _ => None,
    };
    let team_tasks = match &daemon.team {
        Some((seed, resp)) if seed == &current_seed && !resp.tasks.tasks.is_empty() => {
            Some(&resp.tasks.tasks)
        }
        _ => None,
    };

    let active_tab = match daemon.inspector_tab {
        InspectorTab::Overview => InspectorTab::Overview,
        InspectorTab::Tasks if todo_items.is_some() => InspectorTab::Tasks,
        InspectorTab::Team if team_tasks.is_some() => InspectorTab::Team,
        _ => InspectorTab::Overview,
    };

    let mut tab_items = vec![segmented_item(
        "insp-tab-overview",
        "概览",
        active_tab == InspectorTab::Overview,
        &t,
        cx.listener(|daemon, _, _, cx| {
            daemon.inspector_tab = InspectorTab::Overview;
            cx.notify();
        }),
    )];
    if todo_items.is_some() {
        tab_items.push(segmented_item(
            "insp-tab-tasks",
            "任务",
            active_tab == InspectorTab::Tasks,
            &t,
            cx.listener(|daemon, _, _, cx| {
                daemon.inspector_tab = InspectorTab::Tasks;
                cx.notify();
            }),
        ));
    }
    if team_tasks.is_some() {
        tab_items.push(segmented_item(
            "insp-tab-team",
            "团队",
            active_tab == InspectorTab::Team,
            &t,
            cx.listener(|daemon, _, _, cx| {
                daemon.inspector_tab = InspectorTab::Team;
                cx.notify();
            }),
        ));
    }

    let header_left: AnyElement = if tab_items.len() > 1 {
        segmented_strip(tab_items, &t).into_any_element()
    } else {
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                Icon::new(IconName::Activity)
                    .with_size(Size::XSmall)
                    .text_color(t.accent),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child("会话概览"),
            )
            .into_any_element()
    };

    let header = div()
        .h(px(44.))
        .px_3p5()
        .bg(t.surface_subtle)
        .border_b_1()
        .border_color(t.border_subtle)
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(header_left)
        .child(
            div()
                .id("close-inspector")
                .size(px(24.))
                .rounded(px(RADIUS_CHIP))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(t.hover))
                .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.dim))
                .on_click(cx.listener(|daemon, _, _, cx| {
                    daemon.inspector_open = false;
                    cx.notify();
                })),
        );

    let body: AnyElement = match active_tab {
        InspectorTab::Overview => {
            struct OverviewSnapshot {
                title: SharedString,
                status_label: &'static str,
                status_color: gpui_kit::Hsla,
                running: bool,
                cwd: String,
                model_text: String,
                effort_text: String,
                tool_mode: &'static str,
                turns_count: usize,
                tools_count: usize,
                out_tokens: u64,
                total_tokens: u64,
                tok_rate: Option<f64>,
                goal: Option<(String, &'static str, gpui_kit::Hsla, Option<u64>, u64)>,
            }
            let snap = {
                let store = current_entity.read(cx);
                let (status_label, status_color, active) = match store.activity {
                    Some(ActivityState::Running) => ("运行中", t.accent, true),
                    Some(ActivityState::WaitingUser) => ("等待响应", t.warn, true),
                    Some(ActivityState::Cancelling) => ("正在停止", t.warn, true),
                    _ => ("空闲", t.dim, false),
                };
                let cwd = store
                    .settings
                    .as_ref()
                    .and_then(|s| s.meta.cwd.clone())
                    .or_else(|| {
                        store
                            .code_panel
                            .cwd
                            .as_ref()
                            .map(|p| p.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "默认工作目录".to_owned());
                let sel = store
                    .settings
                    .as_ref()
                    .and_then(|s| s.meta.model_selection.as_ref());
                let model_text = sel
                    .map(|m| format!("{} / {}", m.provider_id, m.model_id))
                    .unwrap_or_else(|| "继承全局默认".to_owned());
                let effort_text = sel
                    .and_then(|m| m.options.reasoning_effort.clone())
                    .unwrap_or_else(|| "默认".to_owned());
                let tool_mode = store
                    .settings
                    .as_ref()
                    .map(|s| overview_tool_mode_label(s.meta.tool_mode.as_str()))
                    .unwrap_or("标准");
                let mut out_tokens = 0u64;
                let mut total_tokens = 0u64;
                for entry in &store.entries {
                    if let TimelineEntry::AssistantBlock {
                        usage: Some(usage), ..
                    } = entry
                    {
                        out_tokens += u64::from(usage.completion_tokens);
                        total_tokens += u64::from(usage.total_tokens);
                    }
                }
                let goal = store.goal.as_ref().map(|g| {
                    let (glabel, gcolor) = match g.status {
                        GoalStatus::Active => ("进行中", t.ok),
                        GoalStatus::Paused => ("已暂停", t.warn),
                        GoalStatus::Blocked => ("已受阻", t.bad),
                        GoalStatus::UsageLimited => ("用量熔断", t.warn),
                        GoalStatus::BudgetLimited => ("预算熔断", t.warn),
                        GoalStatus::Complete => ("已完成", t.ok),
                    };
                    (
                        g.objective.clone(),
                        glabel,
                        gcolor,
                        g.token_budget,
                        g.tokens_used,
                    )
                });
                OverviewSnapshot {
                    title: store.display_title(),
                    status_label,
                    status_color,
                    running: active,
                    cwd,
                    model_text,
                    effort_text,
                    tool_mode,
                    turns_count: store.turns.len(),
                    tools_count: store.tools.len(),
                    out_tokens,
                    total_tokens,
                    tok_rate: store.is_running().then(|| store.tok_rate()).flatten(),
                    goal,
                }
            };

            let status_card = div()
                .p_3()
                .rounded(px(RADIUS_CONTROL))
                .bg(t.surface_subtle)
                .border_1()
                .border_color(t.border_subtle)
                .flex()
                .flex_col()
                .gap_1p5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .min_w_0()
                                .child(live_dot("insp-ov-dot", snap.status_color, snap.running))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text)
                                        .truncate()
                                        .child(snap.title),
                                ),
                        )
                        .child(pill(snap.status_label, snap.status_color, &t)),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .text_color(t.faint)
                        .child(Icon::new(IconName::Folder).with_size(Size::XSmall))
                        .child(div().min_w_0().flex_1().truncate().child(snap.cwd)),
                );

            let stats_grid = div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(overview_stat_box("对话回合", snap.turns_count.to_string(), &t))
                        .child(overview_stat_box("工具调用", snap.tools_count.to_string(), &t)),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(overview_stat_box(
                            "输出 Token",
                            format_tok_count(snap.out_tokens),
                            &t,
                        ))
                        .child(overview_stat_box(
                            "总 Token",
                            match snap.tok_rate {
                                Some(rate) => format!(
                                    "{} · {:.0}/s",
                                    format_tok_count(snap.total_tokens),
                                    rate.max(1.0)
                                ),
                                None => format_tok_count(snap.total_tokens),
                            },
                            &t,
                        )),
                );

            let model_card = div()
                .p_3()
                .rounded(px(RADIUS_CONTROL))
                .bg(t.surface_subtle)
                .border_1()
                .border_color(t.border_subtle)
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.dim)
                                .child("模型与策略"),
                        )
                        .child(pill(snap.tool_mode, t.dim, &t)),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text)
                        .truncate()
                        .child(snap.model_text),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.faint)
                                .child(SharedString::from(format!("推理深度：{}", snap.effort_text))),
                        )
                        .child(button(
                            cx,
                            BtnKind::Ghost,
                            "ov-open-session-settings",
                            "调整",
                            &t,
                            cx.listener(|daemon, _, window, cx| {
                                daemon.open_settings(SettingsEditor::Session, window, cx);
                            }),
                        )),
                );

            let goal_card = snap.goal.map(|(obj, glabel, gcolor, budget, used)| {
                let budget_text = match budget {
                    Some(max) => format!("{} / {}", format_tok_count(used), format_tok_count(max)),
                    None => format!("已用 {} tok", format_tok_count(used)),
                };
                div()
                    .p_3()
                    .rounded(px(RADIUS_CONTROL))
                    .bg(t.surface_subtle)
                    .border_1()
                    .border_color(t.border_subtle)
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.dim)
                                    .child("目标模式"),
                            )
                            .child(pill(glabel, gcolor, &t)),
                    )
                    .child(div().text_xs().text_color(t.text).child(obj))
                    .child(div().text_xs().text_color(t.faint).child(budget_text))
            });

            let actions_card = div().flex().gap_2().child(button(
                cx,
                BtnKind::Soft,
                "ov-open-terminal",
                "打开交互终端",
                &t,
                cx.listener(|daemon, _, window, cx| {
                    if !daemon.shell_dock_open {
                        crate::shell_panel::toggle(daemon, window, cx);
                    }
                }),
            ));

            div()
                .id("overview-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_3()
                .flex()
                .flex_col()
                .gap_3()
                .child(status_card)
                .child(stats_grid)
                .child(model_card)
                .children(goal_card)
                .child(actions_card)
                .into_any_element()
        }
        InspectorTab::Tasks => {
            let items = todo_items.expect("active_tab == Tasks");
            let total = items.len();
            let done = items
                .iter()
                .filter(|item| item.status == "completed")
                .count();
            let ratio = if total == 0 {
                0.0
            } else {
                done as f32 / total as f32
            };
            let progress_bar = div()
                .px_3p5()
                .pt_3()
                .pb_2()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_xs()
                        .child(div().text_color(t.dim).child("完成进度"))
                        .child(
                            div()
                                .font_family("monospace")
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(if done == total { t.ok } else { t.text })
                                .child(SharedString::from(format!("{done} / {total}"))),
                        ),
                )
                .child(
                    div()
                        .h(px(6.))
                        .w_full()
                        .rounded_full()
                        .bg(t.surface_subtle)
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(ratio))
                                .rounded_full()
                                .bg(if done == total { t.ok } else { t.accent }),
                        ),
                );

            let mut list = div()
                .id("todo-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_2p5()
                .pb_3()
                .flex()
                .flex_col()
                .gap_1();
            for (idx, item) in items.iter().enumerate() {
                let in_progress = item.status == "in_progress";
                let settled = item.status == "completed" || item.status == "cancelled";
                let icon_el: AnyElement = match item.status.as_str() {
                    "completed" => Icon::new(IconName::CircleCheck)
                        .with_size(Size::XSmall)
                        .text_color(t.ok)
                        .into_any_element(),
                    "cancelled" => Icon::new(IconName::CircleX)
                        .with_size(Size::XSmall)
                        .text_color(t.bad)
                        .into_any_element(),
                    "in_progress" => Icon::new(IconName::LoaderCircle)
                        .with_size(Size::XSmall)
                        .text_color(t.accent)
                        .with_animation(
                            SharedString::from(format!("todo-spin-{idx}")),
                            Animation::new(std::time::Duration::from_millis(900)).repeat(),
                            |icon, val| icon.rotate(gpui_kit::Radians(val * std::f32::consts::TAU)),
                        )
                        .into_any_element(),
                    _ => Icon::new(IconName::Circle)
                        .with_size(Size::XSmall)
                        .text_color(t.faint)
                        .into_any_element(),
                };
                list = list.child(
                    div()
                        .px_2p5()
                        .py_2()
                        .rounded(px(RADIUS_CONTROL))
                        .when(in_progress, |row| {
                            row.bg(t.accent_soft)
                                .border_1()
                                .border_color(soft(t.accent, 0.22))
                        })
                        .when(!in_progress, |row| row.bg(t.surface_subtle))
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(div().pt(px(2.)).flex_shrink_0().child(icon_el))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_xs()
                                .font_weight(if in_progress {
                                    FontWeight::MEDIUM
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(if settled { t.dim } else { t.text })
                                .child(item.title.clone()),
                        ),
                );
            }
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(progress_bar)
                .child(list)
                .into_any_element()
        }
        InspectorTab::Team => {
            let tasks = team_tasks.expect("active_tab == Team");
            let mut list = div()
                .id("team-list")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p_2p5()
                .flex()
                .flex_col()
                .gap_1p5();
            for task in tasks {
                use qaqh_ringing_contract::team::task_types::TaskState;
                let (led, color) = match task.state {
                    TaskState::Open => ("待认领", t.dim),
                    TaskState::Claimed => ("进行中", t.accent),
                    TaskState::Completed | TaskState::Closed => ("已完结", t.ok),
                    TaskState::Cancelled => ("已取消", t.bad),
                };
                let owner = task
                    .owner
                    .as_ref()
                    .map(|actor| {
                        actor
                            .agent_path
                            .as_str()
                            .split('/')
                            .next_back()
                            .unwrap_or("root")
                            .to_owned()
                    })
                    .unwrap_or_else(|| "未认领".into());
                let mut meta = format!("{owner} · epoch {}", task.claim_epoch);
                if !task.depends_on.is_empty() {
                    meta.push_str(&format!(" · 依赖 {}", task.depends_on.len()));
                }
                list = list.child(
                    div()
                        .p_2p5()
                        .rounded(px(RADIUS_CONTROL))
                        .bg(t.surface_subtle)
                        .border_1()
                        .border_color(t.border_subtle)
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(t.text)
                                        .truncate()
                                        .child(task.title.clone()),
                                )
                                .child(pill(led, color, &t)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.faint)
                                .child(SharedString::from(meta)),
                        ),
                );
            }
            list.into_any_element()
        }
    };

    let full_width = 300.0_f32;
    let target_width = if daemon.inspector_open { full_width } else { 0.0 };
    let width = spring_f32(
        "inspector-dock-width",
        target_width,
        spring_panel_policy(),
        window,
        cx,
    )
    .max(0.0);
    if !daemon.inspector_open && width <= 0.5 {
        return None;
    }
    let ratio = (width / full_width).clamp(0.0, 1.0);
    let inner = div()
        .w(px(full_width))
        .h_full()
        .pl_2()
        .flex_shrink_0()
        .child(
            stage_surface(&t)
                .w_full()
                .h_full()
                .flex()
                .flex_col()
                .child(header)
                .child(body),
        );

    Some(
        div()
            .h_full()
            .w(px(width))
            .opacity(ratio)
            .flex_shrink_0()
            .overflow_hidden()
            .child(inner)
            .into_any_element(),
    )
}

/// 居中模态设置工作台(DESIGN LAW v3.0):
/// 左侧 200px 导航目录;右侧 720px 内容区,支持主题风格/动效三档与全项内联编辑。
pub(crate) fn settings_page(
    daemon: &mut DaemonStore,
    cx: &mut Context<DaemonStore>,
) -> Div {
    let t = *cx.colors();
    let settings = settings::get(cx).clone();
    let appearance = &settings.appearance;
    let typo = &settings.typography;
    let behavior = &settings.behavior;

    let rail_header = div()
        .px_2p5()
        .pt_1()
        .pb_2()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text)
        .child("设置");

    let archived_count = daemon.sessions.iter().filter(|row| row.archived).count();
    let nav_items = [
        (SettingsEditor::Local, IconName::SlidersHorizontal, "本机偏好", None),
        (SettingsEditor::Config, IconName::Shield, "全局策略", None),
        (SettingsEditor::Catalog, IconName::Cpu, "服务商与模型", None),
        (SettingsEditor::Accounts, IconName::CircleUser, "账号与额度", None),
        (SettingsEditor::Session, IconName::MessageSquare, "当前会话", None),
        (SettingsEditor::Workspaces, IconName::FolderKanban, "工作区", None),
        (
            SettingsEditor::Archived,
            IconName::Archive,
            "已归档会话",
            (archived_count > 0).then(|| SharedString::from(archived_count.to_string())),
        ),
        (SettingsEditor::About, IconName::Info, "关于", None),
    ]
    .into_iter()
    .map(|(mode, icon, label, badge)| {
        let active = daemon.settings_editor == mode;
        nav_pill(
            SharedString::from(format!("settings-nav-{mode:?}")),
            icon,
            label,
            badge,
            active,
            &t,
            cx.listener(move |daemon: &mut DaemonStore, _, window, cx| {
                daemon.open_settings(mode, window, cx);
                if mode == SettingsEditor::Accounts {
                    daemon.refresh_accounts();
                }
            }),
        )
    });

    let left_rail = div()
        .w(px(204.))
        .h_full()
        .flex_shrink_0()
        .bg(t.surface_subtle)
        .border_r_1()
        .border_color(t.border_subtle)
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .child(rail_header)
        .children(nav_items)
        .child(div().flex_1())
        .child(
            div()
                .px_2()
                .pt_2()
                .border_t_1()
                .border_color(t.border_subtle)
                .text_xs()
                .text_color(t.faint)
                .child(SharedString::from(format!(
                    "配置路径：{}",
                    settings::config_path().display()
                ))),
        );

    let (page_title, page_desc) = match daemon.settings_editor {
        SettingsEditor::Local => ("本机偏好", "管理外观风格、主题明暗、GPU 物理动效与阅读排版。"),
        SettingsEditor::Config => ("全局策略", "所有项修改后即时生效，实时同步至本地 daemon。"),
        SettingsEditor::Catalog => ("服务商与模型", "配置模型接口端点、凭据来源及默认推理模型。"),
        SettingsEditor::Accounts => ("账号与额度", "通过系统浏览器授权登录并管理可用额度。"),
        SettingsEditor::Session => ("当前会话", "覆盖调整当前激活会话的模型、推理强度与工具权限。"),
        SettingsEditor::Workspaces => ("工作区", "管理桌面工作区目录分组；删除分组不会删除磁盘文件。"),
        SettingsEditor::Archived => ("已归档会话", "浏览、恢复或彻底移除已归档的工程会话。"),
        SettingsEditor::About => ("关于", "查看版本、RingingV2 协议修订号与运行环境信息。"),
    };

    let is_local = daemon.settings_editor == SettingsEditor::Local;
    let body: Div = if is_local {
        let theme_buttons: Vec<AnyElement> = [
            (ThemeMode::System, "跟随系统"),
            (ThemeMode::Light, "亮色"),
            (ThemeMode::Dark, "暗色"),
        ]
        .iter()
        .map(|(mode, label)| {
            let mode = *mode;
            segmented_item(
                format!("theme-{}", label),
                *label,
                appearance.theme_mode == mode,
                &t,
                cx.listener(move |_, _, _, cx| {
                    settings::update(cx, |content| content.appearance.theme_mode = mode);
                }),
            )
        })
        .collect();

        let preset_buttons: Vec<AnyElement> = [ThemePreset::WarmPaper, ThemePreset::PureStudio]
            .into_iter()
            .map(|preset| {
                segmented_item(
                    format!("preset-{preset:?}"),
                    preset.label(),
                    appearance.theme_preset == preset,
                    &t,
                    cx.listener(move |_, _, _, cx| {
                        settings::update(cx, |content| content.appearance.theme_preset = preset);
                    }),
                )
            })
            .collect();

        let motion_buttons: Vec<AnyElement> = [
            MotionLevel::Full,
            MotionLevel::Reduced,
            MotionLevel::Off,
        ]
        .into_iter()
        .map(|level| {
            segmented_item(
                format!("motion-{level:?}"),
                level.label(),
                appearance.motion_level == level,
                &t,
                cx.listener(move |_, _, _, cx| {
                    settings::update(cx, |content| content.appearance.motion_level = level);
                }),
            )
        })
        .collect();

        let densities = [Density::Compact, Density::Standard, Density::Comfortable];
        let density_buttons: Vec<AnyElement> = densities
            .iter()
            .map(|density| {
                let density = *density;
                segmented_item(
                    format!("density-{}", density.label()),
                    density.label(),
                    appearance.density == density,
                    &t,
                    cx.listener(move |_, _, _, cx| {
                        settings::update(cx, |content| content.appearance.density = density);
                    }),
                )
            })
            .collect();

        let font_size_display = SharedString::from(format!("{:.0} px", typo.ui_font_size));
        let font_size_stepper = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(step_button(
                "font-size-minus",
                IconName::Minus,
                &t,
                cx.listener(|_, _, _, cx| {
                    settings::update(cx, |content| {
                        content.typography.ui_font_size =
                            (content.typography.ui_font_size - 1.0).max(12.0);
                    });
                }),
            ))
            .child(
                div()
                    .min_w(px(48.))
                    .text_center()
                    .text_xs()
                    .text_color(t.text)
                    .child(font_size_display),
            )
            .child(step_button(
                "font-size-plus",
                IconName::Plus,
                &t,
                cx.listener(|_, _, _, cx| {
                    settings::update(cx, |content| {
                        content.typography.ui_font_size =
                            (content.typography.ui_font_size + 1.0).min(20.0);
                    });
                }),
            ));

        let font_input = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().w(px(200.)).child(Input::new(&daemon.font_field).small()))
            .child(button(
                cx,
                BtnKind::Soft,
                "apply-ui-font",
                "应用",
                &t,
                cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                    let text = daemon.font_field.read(cx).value().trim().to_string();
                    settings::update(cx, |content| content.typography.ui_font_family = text);
                }),
            ));

        let code_size_display = SharedString::from(format!("{:.0} px", typo.code_font_size));
        let code_size_stepper = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(step_button(
                "code-size-minus",
                IconName::Minus,
                &t,
                cx.listener(|_, _, _, cx| {
                    settings::update(cx, |content| {
                        content.typography.code_font_size =
                            (content.typography.code_font_size - 1.0).max(10.0);
                    });
                }),
            ))
            .child(
                div()
                    .min_w(px(48.))
                    .text_center()
                    .text_xs()
                    .text_color(t.text)
                    .child(code_size_display),
            )
            .child(step_button(
                "code-size-plus",
                IconName::Plus,
                &t,
                cx.listener(|_, _, _, cx| {
                    settings::update(cx, |content| {
                        content.typography.code_font_size =
                            (content.typography.code_font_size + 1.0).min(16.0);
                    });
                }),
            ));

        let line_height_display = SharedString::from(format!("{:.2}", typo.prose_line_height));
        let line_height_stepper = div()
            .flex()
            .items_center()
            .gap_1p5()
            .child(step_button(
                "line-height-minus",
                IconName::Minus,
                &t,
                cx.listener(|_, _, _, cx| {
                    settings::update(cx, |content| {
                        content.typography.prose_line_height =
                            (content.typography.prose_line_height - 0.05).max(1.2);
                    });
                }),
            ))
            .child(
                div()
                    .min_w(px(48.))
                    .text_center()
                    .text_xs()
                    .text_color(t.text)
                    .child(line_height_display),
            )
            .child(step_button(
                "line-height-plus",
                IconName::Plus,
                &t,
                cx.listener(|_, _, _, cx| {
                    settings::update(cx, |content| {
                        content.typography.prose_line_height =
                            (content.typography.prose_line_height + 0.05).min(2.0);
                    });
                }),
            ));

        let width_buttons: Vec<AnyElement> = [640.0, 760.0, 850.0, 960.0]
            .into_iter()
            .map(|width| {
                segmented_item(
                    format!("content-width-{width:.0}"),
                    format!("{width:.0}"),
                    (typo.content_width - width).abs() < 1.0,
                    &t,
                    cx.listener(move |_, _, _, cx| {
                        settings::update(cx, |content| content.typography.content_width = width);
                    }),
                )
            })
            .collect();

        let terminal_buttons: Vec<AnyElement> = TerminalScheme::ALL
            .into_iter()
            .map(|scheme| {
                segmented_item(
                    format!("term-scheme-{scheme:?}"),
                    scheme.label(),
                    appearance.terminal_scheme == scheme,
                    &t,
                    cx.listener(move |_, _, _, cx| {
                        settings::update(cx, |content| content.appearance.terminal_scheme = scheme);
                    }),
                )
            })
            .collect();

        let follow_switch = Switch::new("local-stream-follow")
            .checked(behavior.stream_follow)
            .small()
            .on_change(|checked, _, cx| {
                let val = *checked;
                settings::update(cx, |content| content.behavior.stream_follow = val);
            })
            .into_any_element();

        let tail_buttons: Vec<AnyElement> = [(4096usize, "4KB"), (16 * 1024, "16KB"), (64 * 1024, "64KB")]
            .into_iter()
            .map(|(bytes, label)| {
                segmented_item(
                    format!("tail-{label}"),
                    label,
                    behavior.output_tail_max == bytes,
                    &t,
                    cx.listener(move |_, _, _, cx| {
                        settings::update(cx, |content| content.behavior.output_tail_max = bytes);
                    }),
                )
            })
            .collect();

        let reload_control: AnyElement = if daemon.current.is_some() {
            button(
                cx,
                BtnKind::Soft,
                "reload-agent-config",
                "重载当前会话",
                &t,
                cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                    if let Some(current) = daemon.current.clone() {
                        let session_id = current.read(cx).session_id.clone();
                        let _ = daemon.requests.unbounded_send(BridgeRequest::Command {
                            target: qaqh_client::CommandTarget::Session { session_id },
                            command: qaqh_client::RingingCommand::AgentReloadConfig,
                        });
                    }
                    cx.notify();
                }),
            )
        } else {
            div()
                .text_xs()
                .text_color(t.faint)
                .child("请先打开任意会话")
                .into_any_element()
        };

        let reconnect_control = button(
            cx,
            BtnKind::Soft,
            "reconnect-daemon",
            "重新连接",
            &t,
            cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                daemon.connection = ConnectionState::Connecting;
                let _ = daemon.requests.unbounded_send(BridgeRequest::Reconnect);
                cx.notify();
            }),
        );

        let diag_control = button(
            cx,
            BtnKind::Ghost,
            "copy-diagnostics",
            "复制诊断报告",
            &t,
            cx.listener(|_, _, window, cx| {
                cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(
                    qaqh_gpui_wire::diagnostics::report(),
                ));
                window.push_notification(
                    gpui_kit::component::notification::Notification::success("诊断报告已复制"),
                    cx,
                );
            }),
        );

        div()
            .flex()
            .flex_col()
            .gap_5()
            .child(section_card(
                "外观与材质",
                vec![
                    setting_row(
                        "界面风格",
                        "暖纸工作台或纯白极简舞台,暗色模式自动适配",
                        segmented_strip(preset_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "主题模式",
                        "支持随 Windows 11 系统自动切换亮暗材质",
                        segmented_strip(theme_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "界面动效",
                        "120fps 物理弹簧与手风琴过渡，可一键精简或关闭",
                        segmented_strip(motion_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "时间线留白",
                        "调整消息段落间的呼吸节奏",
                        segmented_strip(density_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "终端配色方案",
                        "应用于底部交互终端与命令输出面板的 ANSI 调色板",
                        segmented_strip(terminal_buttons, &t).into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ))
            .child(section_card(
                "阅读与排版",
                vec![
                    setting_row(
                        "界面字号",
                        "全局界面文字缩放基准",
                        font_size_stepper.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "界面字体",
                        "留空即使用系统默认字体，输入后回车或点应用",
                        font_input.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "代码字号",
                        "代码块与内置终端文字大小",
                        code_size_stepper.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "正文行高",
                        "消息段落行高倍数",
                        line_height_stepper.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "阅读列宽 (px)",
                        "主时间线与输入岛居中最大宽度",
                        segmented_strip(width_buttons, &t).into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ))
            .child(section_card(
                "交互与输出",
                vec![
                    setting_row(
                        "自动跟随底部",
                        "流式生成时自动贴底滚动，上滚即暂停跟随",
                        follow_switch,
                        &t,
                    ),
                    setting_row(
                        "工具输出尾窗上限",
                        "单次工具输出超限时保留尾部内容",
                        segmented_strip(tail_buttons, &t).into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ))
            .child(section_card(
                "Daemon 运维",
                vec![
                    setting_row("热重载会话配置", "让当前代理重新读取服务商与凭据", reload_control, &t),
                    setting_row("重连本地 Daemon", "连接中断时手动重建长连接", reconnect_control, &t),
                    setting_row("导出诊断信息", "复制脱敏诊断快照到剪贴板（不含对话正文）", diag_control, &t),
                ],
                &t,
            ))
    } else {
        crate::settings_ui::render(daemon, cx, &t)
    };

    let header_bar = div()
        .flex()
        .items_start()
        .justify_between()
        .gap_4()
        .pb_4()
        .mb_5()
        .border_b_1()
        .border_color(t.border_subtle)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child(page_title),
                )
                .child(div().text_xs().text_color(t.dim).child(page_desc)),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .children(is_local.then(|| {
                    button(
                        cx,
                        BtnKind::Ghost,
                        "reset-settings",
                        "恢复默认设置",
                        &t,
                        cx.listener(|_, _, _, cx| {
                            settings::update(cx, |content| {
                                *content = settings::SettingsContent::default();
                            });
                        }),
                    )
                }))
                .child(
                    div()
                        .id("close-settings")
                        .size(px(28.))
                        .rounded(px(RADIUS_CHIP))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|style| style.bg(t.hover))
                        .child(Icon::new(IconName::X).with_size(Size::Small).text_color(t.dim))
                        .on_click(cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                            daemon.settings_open = false;
                            daemon.settings_form = None;
                            cx.notify();
                        })),
                ),
        );

    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .child(left_rail)
        .child(
            div()
                .id(format!(
                    "settings-scroll-{:?}-{}",
                    daemon.settings_editor,
                    daemon.settings_form.is_some()
                ))
                .flex_1()
                .min_w_0()
                .min_h_0()
                .overflow_y_scroll()
                .px_8()
                .pt_6()
                .pb_10()
                .child(
                    div()
                        .max_w(px(720.))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .child(header_bar)
                        .child(body),
                ),
        )
}

/// daemon 权限档位。
pub(crate) fn permission_row(
    daemon: &DaemonStore,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let control = match &daemon.config {
        Some(config) => {
            let buttons = [
                (1u8, "只读"),
                (2, "工作区写"),
                (3, "跳过授权"),
                (4, "沙箱运行"),
            ]
            .into_iter()
            .map(|(level, label)| {
                segmented_button(
                    format!("perm-{level}"),
                    label,
                    config.permission_level == level,
                    t,
                    cx.listener(move |daemon: &mut DaemonStore, _, _, _| {
                        let _ = daemon.requests.unbounded_send(BridgeRequest::SaveConfig(
                            ConfigPatch {
                                permission_level: Some(u64::from(level)),
                                ..Default::default()
                            },
                        ));
                    }),
                )
            });
            div().flex().flex_wrap().gap_1().children(buttons)
        }
        None => div().text_xs().text_color(t.dim).child("读取中…"),
    };
    setting_row("权限档位", "daemon 全局默认执行与文件访问策略", control.into_any_element(), t)
}
