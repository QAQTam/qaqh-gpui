//! 融合自定义 TitleBar(42px,透出 Win11 Mica):
//! 左侧 = 侧栏弹簧切换钮 + 当前会话面包屑(呼吸光点 / 工作区 / 清洗标题 / Goal / 封箱 / tok·s);
//! 右侧 = 任务板切换胶囊 + 终端切换胶囊 + 主题切换 + 设置(紧接系统三键)。

use gpui_kit::prelude::*;
use gpui_kit::{Context, FontWeight, MouseButton, SharedString, div, px};
use gpui_kit::component::TitleBar;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::soft;
use qaqh_gpui_kit::motion::live_dot;
use qaqh_gpui_kit::primitives::titlebar_pill;
use qaqh_gpui_kit::settings;
use qaqh_gpui_kit::theme::{Palette, ThemeMode};
use qaqh_gpui_session::store::SettingsEditor;
use qaqh_ringing_contract::fact::ActivityState;

use crate::daemon::{DaemonStore, PrimarySection};

pub(crate) fn render(
    daemon: &DaemonStore,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> impl IntoElement {
    let anchors = daemon.handoff_anchors.clone();
    let sidebar_btn = div()
        .relative()
        .child(titlebar_pill(
            "tb-sidebar",
            IconName::PanelLeft,
            None,
            None,
            daemon.sidebar_open,
            if daemon.sidebar_open {
                "收起侧栏 (Ctrl+B)"
            } else {
                "展开侧栏 (Ctrl+B)"
            },
            t,
            cx.listener(|daemon, _, _, cx| {
                daemon.sidebar_open = !daemon.sidebar_open;
                cx.notify();
            }),
        ))
        .child(qaqh_gpui_session::handoff::anchor_probe(move |bounds| {
            anchors.record_titlebar_toggle(bounds);
        }));

    let mut left = div()
        .flex()
        .items_center()
        .gap_2()
        .min_w_0()
        .child(sidebar_btn);

    if daemon.settings_open {
        left = left.child(brand_crumb(Some("设置"), t));
    } else if daemon.section == PrimarySection::Tools {
        left = left.child(brand_crumb(Some("工具"), t));
    } else if daemon.section == PrimarySection::Team {
        left = left.child(brand_crumb(Some("团队"), t));
    } else if daemon.section == PrimarySection::Access {
        left = left.child(brand_crumb(Some("接入"), t));
    } else if daemon.new_session_open {
        left = left.child(brand_crumb(Some("新对话"), t));
    } else if let Some(session_entity) = &daemon.current {
        let store = session_entity.read(cx);
        let dot_color = match store.activity {
            Some(ActivityState::Running) => t.accent,
            Some(ActivityState::WaitingUser | ActivityState::Cancelling) => t.warn,
            _ => match store.last_turn_tone() {
                Some(true) => t.ok,
                Some(false) => t.bad,
                None => t.dim,
            },
        };
        left = left
            .child(live_dot("tb-session", dot_color, store.is_running()))
            .children(store.workspace_tail().map(|ws| {
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().text_xs().text_color(t.dim).child(ws))
                    .child(div().text_xs().text_color(t.faint).child("/"))
            }))
            .child(
                div()
                    .max_w(px(340.))
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text)
                    .child(store.display_title()),
            )
            .children(store.goal.clone().map(|goal| {
                qaqh_gpui_session::goal::goal_badge(&goal, session_entity, t).into_any_element()
            }))
            .children(store.handoff_seal.as_ref().map(|seal| {
                let target = session_entity.clone();
                div()
                    .id("tb-handoff-seal")
                    .h(px(24.))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2p5()
                    .rounded_full()
                    .bg(soft(t.ok, 0.12))
                    .border_1()
                    .border_color(soft(t.ok, 0.24))
                    .text_xs()
                    .text_color(t.ok)
                    .cursor_pointer()
                    .hover(|style| style.bg(soft(t.ok, 0.2)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, _, cx| {
                        let _ = target.update(cx, |store, cx| {
                            store.open_handoff_successor(cx);
                            cx.notify();
                        });
                    })
                    .child(
                        Icon::new(IconName::PackageCheck)
                            .with_size(Size::XSmall)
                            .text_color(t.ok),
                    )
                    .child(SharedString::from(format!("已封箱 · {}", seal.work_summary)))
                    .into_any_element()
            }))
            .children(
                store
                    .is_running()
                    .then(|| store.tok_rate())
                    .flatten()
                    .map(|rate| {
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(t.accent_soft)
                            .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                            .text_xs()
                            .text_color(t.accent)
                            .child(SharedString::from(format!("{:.0} tok/s", rate.max(1.))))
                            .into_any_element()
                    }),
            );
    } else {
        left = left.child(brand_crumb(None, t));
    }

    let current_seed = daemon
        .current
        .as_ref()
        .map(|current| current.read(cx).session_id.clone());
    let todo_counts = match (&daemon.todos, &current_seed) {
        (Some((seed, items)), Some(cur)) if seed == cur && !items.is_empty() => {
            let done = items.iter().filter(|i| i.status == "completed").count();
            Some((done, items.len()))
        }
        _ => None,
    };
    let team_counts = match (&daemon.team, &current_seed) {
        (Some((seed, resp)), Some(cur)) if seed == cur && !resp.tasks.tasks.is_empty() => {
            let tasks = &resp.tasks.tasks;
            let done = tasks
                .iter()
                .filter(|task| {
                    matches!(
                        task.state,
                        qaqh_ringing_contract::team::task_types::TaskState::Completed
                            | qaqh_ringing_contract::team::task_types::TaskState::Closed
                    )
                })
                .count();
            Some((done, tasks.len()))
        }
        _ => None,
    };

    let in_session = daemon.section == PrimarySection::Home
        && !daemon.settings_open
        && !daemon.new_session_open
        && daemon.current.is_some();

    let todo_card_open = daemon
        .current
        .as_ref()
        .is_some_and(|current| current.read(cx).todo_card_open);
    let todo_btn = (in_session && todo_counts.is_some()).then(|| {
        let (done, total) = todo_counts.unwrap();
        titlebar_pill(
            "tb-todo",
            IconName::ListChecks,
            Some("任务".into()),
            Some(SharedString::from(format!("{done}/{total}"))),
            todo_card_open,
            "显示/隐藏任务清单卡片",
            t,
            cx.listener(|daemon, _, _, cx| {
                if let Some(current) = daemon.current.clone() {
                    current.update(cx, |store, cx| {
                        store.todo_card_open = !store.todo_card_open;
                        cx.notify();
                    });
                }
                cx.notify();
            }),
        )
    });

    let team_btn = (in_session && team_counts.is_some()).then(|| {
        let (done, total) = team_counts.unwrap();
        titlebar_pill(
            "tb-inspector",
            IconName::Users,
            Some("团队".into()),
            Some(SharedString::from(format!("{done}/{total}"))),
            daemon.inspector_open,
            "收起/展开团队任务面板",
            t,
            cx.listener(|daemon, _, _, cx| {
                daemon.inspector_open = !daemon.inspector_open;
                cx.notify();
            }),
        )
    });

    let shell_count = daemon.shells.len();
    let terminal_btn = titlebar_pill(
        "tb-terminal",
        IconName::SquareTerminal,
        Some("终端".into()),
        (shell_count > 0).then(|| SharedString::from(shell_count.to_string())),
        daemon.shell_dock_open,
        "切换底部交互终端",
        t,
        cx.listener(|daemon, _, window, cx| {
            crate::shell_panel::toggle(daemon, window, cx);
        }),
    );

    let mode = settings::get(cx).appearance.theme_mode;
    let theme_icon = match mode {
        ThemeMode::System => IconName::Monitor,
        ThemeMode::Light => IconName::Sun,
        ThemeMode::Dark => IconName::Moon,
    };
    let theme_tip = format!("主题：{}（点击切换）", mode.label());
    let theme_btn = titlebar_pill(
        "tb-theme",
        theme_icon,
        None,
        None,
        false,
        theme_tip,
        t,
        cx.listener(|_, _, _, cx| {
            let next = settings::get(cx).appearance.theme_mode.next();
            settings::update(cx, |content| content.appearance.theme_mode = next);
        }),
    );

    let settings_btn = titlebar_pill(
        "tb-settings",
        IconName::Settings,
        Some("设置".into()),
        None,
        daemon.settings_open,
        "打开设置 (Ctrl+,)",
        t,
        cx.listener(|daemon, _, window, cx| {
            if daemon.settings_open {
                daemon.settings_open = false;
                daemon.settings_form = None;
                cx.notify();
            } else {
                daemon.open_settings(SettingsEditor::Local, window, cx);
            }
        }),
    );

    let right = div()
        .flex()
        .items_center()
        .gap_1p5()
        .pr_2()
        .flex_shrink_0()
        .children(todo_btn)
        .children(team_btn)
        .child(terminal_btn)
        .child(theme_btn)
        .child(settings_btn);

    TitleBar::new()
        .h(px(42.))
        .bg(gpui_kit::transparent_black())
        .border_b_0()
        .child(left)
        .child(right)
}

fn brand_crumb(sub: Option<&'static str>, t: &Palette) -> gpui_kit::Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            Icon::new(IconName::Sparkles)
                .with_size(Size::XSmall)
                .text_color(t.text),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.text)
                .child("QAQHarness"),
        )
        .children(sub.map(|name| {
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(div().text_xs().text_color(t.faint).child("/"))
                .child(div().text_xs().text_color(t.dim).child(name))
        }))
}
