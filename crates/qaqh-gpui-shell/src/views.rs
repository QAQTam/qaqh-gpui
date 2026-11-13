//! 元素构建(M3):主题 token 化 + 分组侧栏/时间线/composer/交互应答卡/todo 右栏。
//! `Entity<T: Render>` 即元素:DaemonStore 为根,SessionStore 经实体直接挂载。
//! 颜色一律取 theme::Palette 语义槽;输入框用 gpui-component 的 Input/Textarea
//! (状态实体 + 组件渲染,IME/选区/剪贴板由引擎承担)。

use std::collections::HashSet;

use gpui::prelude::*;
use gpui::AnyElement;
use gpui::ClickEvent;
use gpui::Context;
use gpui::Div;
use gpui::Entity;
use gpui::FontWeight;
use gpui::Hsla;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::KeyDownEvent;
use gpui::ParentElement;
use gpui::Render;
use gpui::FollowMode;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::div;
use gpui::list;
use gpui::px;
use gpui::transparent_black;
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::dialog::Dialog;
use gpui_component::input::{Input, Textarea};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{Sizable, Size, WindowExt};
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::ActorKind;
use qaqh_ringing_contract::fact::InputPurpose;
use qaqh_ringing_contract::fact::ContentValue;
use qaqh_ringing_contract::fact::InteractionKind;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::state::ExecSessionState;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallState;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_types::ToolCallOutcome;
use qaqh_config_api::ConfigPatch;

use qaqh_gpui_wire::ask;
use qaqh_gpui_wire::bridge::BridgeRequest;
use qaqh_gpui_kit::settings;
use qaqh_gpui_kit::settings::Density;
use crate::daemon::DaemonStore;
use qaqh_gpui_session::store::SessionRow;
use qaqh_gpui_session::store::{ArgStream, SessionStore, SettingsEditor};
use qaqh_gpui_session::store::StreamState;
use qaqh_gpui_kit::theme::ActiveTheme;
use qaqh_gpui_kit::theme::Palette;
use qaqh_gpui_kit::theme::ThemeMode;
use qaqh_gpui_kit::primitives::{action_button, section_card, section_header, setting_row, segmented_button, step_button, status_dot_colored};
use qaqh_gpui_session::timeline::status_dot;
use qaqh_gpui_session::store::{next_model, next_option, model_effort_levels};

/// 侧栏条目:选中 = accent 左边条 + selected 底色(借鉴 zed project_panel
/// 的「背景 + 侧边状态边」模式,不搞圆角高亮)。
/// `tone` = 当前加载会话最近回合的权威终态:Some(成功?)覆盖 run-status 点
/// (electron 六态点的「终态绿/红」维度);未加载会话 None。
pub(crate) fn sidebar_row(
    row: &SessionRow,
    selected: bool,
    tone: Option<bool>,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let title = if row.title.is_empty() {
        "(无标题)".to_string()
    } else {
        row.title.clone()
    };
    div()
        .id(SharedString::from(format!("sess-{}", row.id)))
        .cursor_pointer()
        .on_click(listener)
        .flex()
        .flex_col()
        .gap_0p5()
        .pl_2p5()
        .pr_3()
        .py_2()
        .border_l_2()
        .border_color(if selected { t.accent } else { transparent_black() })
        .when(selected, |style| style.bg(t.selected))
        .hover(|style| style.bg(t.hover))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(match tone {
                    Some(true) => status_dot_colored("完成", t.ok, t),
                    Some(false) => status_dot_colored("失败", t.bad, t),
                    None => status_dot(&row.status, t),
                })
        .child(
            div()
                .text_sm()
                .text_color(t.text)
                .child(title),
        ),
        )
        .child(div().text_xs().text_color(t.dim).child(row.updated_at.clone()))
}

/// 右栏任务 dock:站台发车看板(Codex portalLayout 口径)——hero「正在服务」
/// 带完成计数,单行条目 + 状态 LED 词 + 路线色带。无数据/无任务时不渲染。
pub(crate) fn todo_dock(daemon: &mut DaemonStore, cx: &mut Context<DaemonStore>) -> Option<Div> {
    let t = *cx.colors();
    let seed = daemon
        .current
        .as_ref()
        .map(|current| current.read(cx).session_id.clone());
    let todos = match (&daemon.todos, &seed) {
        (Some((cached_seed, items)), Some(current)) if cached_seed == current => Some(items),
        _ => None,
    };
    let Some(items) = todos else {
        return None;
    };
    if items.is_empty() {
        return None;
    }
    let done = items.iter().filter(|item| item.status == "completed").count();
    let serving = items.iter().filter(|item| item.status == "in_progress").count();
    let dock = div()
        .w(px(260.))
        .h_full()
        .flex()
        .flex_col()
        .bg(t.panel)
        .border_l_1()
        .border_color(t.border)
        // hero:现在开行的任务(进行中计数),发车看板的站牌位。
        .child(
            div()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(t.border)
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.accent)
                        .child(if serving > 0 { "正在服务" } else { "任务" }),
                )
                .child(
                    div()
                        .px_1p5()
                        .rounded_full()
                        .text_xs()
                        .bg(qaqh_gpui_kit::kit::soft(t.faint, 0.14))
                        .text_color(t.dim)
                        .child(SharedString::from(format!("{done}/{}", items.len()))),
                ),
        );
    let mut list = div()
        .id("todo-list")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col();
    for item in items {
        let (led, color) = match item.status.as_str() {
            "in_progress" => ("进行中", t.accent),
            "completed" => ("已完成", t.ok),
            "cancelled" => ("已取消", t.bad),
            _ => ("待处理", t.dim),
        };
        let settled = item.status == "completed" || item.status == "cancelled";
        list = list.child(
            // 单行条目:左侧路线色带 + 标题截断 + 右侧 LED 词。
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2p5()
                .py_1p5()
                .border_l_2()
                .border_color(color)
                .child(
                    div()
                        .text_sm()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(if settled { t.dim } else { t.text })
                        .child(item.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .flex_shrink_0()
                        .text_color(color)
                        .child(SharedString::from(led)),
                ),
        );
    }
    Some(dock.child(list))
}

/// 右栏团队 dock(ADR-0014):当前会话任务板的单行 LED 条目。
/// 花名册不渲染(粗投影,精确状态在 session.list);任务板为空/快照不属于
/// 当前会话时不渲染。依赖数与认领代纪按施工单 §一·1 口径展示。
pub(crate) fn team_dock(daemon: &DaemonStore, cx: &Context<DaemonStore>) -> Option<Div> {
    let t = *cx.colors();
    let current_seed = daemon
        .current
        .as_ref()
        .map(|current| current.read(cx).session_id.clone());
    let (seed, response) = daemon.team.as_ref()?;
    if current_seed.as_deref() != Some(seed.as_str()) || response.tasks.tasks.is_empty() {
        return None;
    }
    let tasks = &response.tasks.tasks;
    let done = tasks.iter().filter(|task| matches!(task.state, qaqh_ringing_contract::team::task_types::TaskState::Completed | qaqh_ringing_contract::team::task_types::TaskState::Closed)).count();
    let dock = div()
        .w(px(260.))
        .h_full()
        .flex()
        .flex_col()
        .bg(t.panel)
        .border_l_1()
        .border_color(t.border)
        .child(
            div()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(t.border)
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.accent)
                        .child("任务板"),
                )
                .child(
                    div()
                        .px_1p5()
                        .rounded_full()
                        .text_xs()
                        .bg(qaqh_gpui_kit::kit::soft(t.faint, 0.14))
                        .text_color(t.dim)
                        .child(SharedString::from(format!("{done}/{}", tasks.len()))),
                ),
        );
    let mut list = div()
        .id("team-list")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col();
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
            .map(|actor| actor.agent_path.as_str().split('/').next_back().unwrap_or("root").to_owned())
            .unwrap_or_else(|| "未认领".into());
        let mut meta = format!("{owner} · epoch {}", task.claim_epoch);
        if !task.depends_on.is_empty() {
            meta.push_str(&format!(" · 依赖 {}", task.depends_on.len()));
        }
        list = list.child(
            // 条目 = 两行列:标题 + LED,下面 owner/epoch/依赖注记;路线色带挂左缘。
            div()
                .flex()
                .flex_col()
                .border_l_2()
                .border_color(color)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2p5()
                        .py_1p5()
                        .child(
                            div()
                                .text_sm()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(t.text)
                                .child(task.title.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .flex_shrink_0()
                                .text_color(color)
                                .child(SharedString::from(led)),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .px_2p5()
                        .pb_1()
                        .text_color(t.faint)
                        .child(SharedString::from(meta)),
                ),
        );
    }
    Some(dock.child(list))
}

/// 设置页(M6):本地偏好(外观/排版/行为)。结构借鉴 zed settings_ui 的
/// 「分区头 + 行(左标题描述、右控件)+ 行间分隔线」;数据模型是无状态 UI:
/// 控件不存值,全部经 settings::update(落盘 + refresh_windows)。
pub(crate) fn settings_page(
    daemon: &mut DaemonStore,
    cx: &mut Context<DaemonStore>,
) -> Div {
    let t = *cx.colors();
    let settings = settings::get(cx).clone();
    let appearance = &settings.appearance;
    let typo = &settings.typography;
    let behavior = &settings.behavior;

    // —— 外观 ——
    let theme_buttons: Vec<AnyElement> = [
        (ThemeMode::System, "跟随系统"),
        (ThemeMode::Light, "亮色"),
        (ThemeMode::Dark, "暗色"),
    ]
    .iter()
    .map(|(mode, label)| {
        let mode = *mode;
        segmented_button(
            format!("theme-{}", label),
            *label,
            appearance.theme_mode == mode,
            &t,
            cx.listener(move |_, _, _, cx| {
                settings::update(cx, |content| content.appearance.theme_mode = mode);
            }),
        )
        .into_any_element()
    })
    .collect();

    let densities = [Density::Compact, Density::Standard, Density::Comfortable];
    let density_buttons: Vec<AnyElement> = densities
        .iter()
        .map(|density| {
            let density = *density;
            segmented_button(
                format!("density-{}", density.label()),
                density.label(),
                appearance.density == density,
                &t,
                cx.listener(move |_, _, _, cx| {
                    settings::update(cx, |content| content.appearance.density = density);
                }),
            )
            .into_any_element()
        })
        .collect();

    // —— 排版 ——
    let font_size_display = SharedString::from(format!("{:.0}", typo.ui_font_size));
    let font_size_stepper = div()
        .flex()
        .items_center()
        .gap_1()
        .child(step_button(
            "font-size-minus",
            "−",
            &t,
            cx.listener(|_, _, _, cx| {
                settings::update(cx, |content| {
                    content.typography.ui_font_size =
                        (content.typography.ui_font_size - 1.0).max(12.0);
                });
            }),
        ))
        .child(div().min_w(px(44.)).text_xs().text_color(t.dim).child(font_size_display))
        .child(step_button(
            "font-size-plus",
            "+",
            &t,
            cx.listener(|_, _, _, cx| {
                settings::update(cx, |content| {
                    content.typography.ui_font_size =
                        (content.typography.ui_font_size + 1.0).min(20.0);
                });
            }),
        ));

    // 字体输入框:Enter 应用由 DaemonStore 的事件订阅驱动。
    let font_input = div().w(px(220.)).child(Input::new(&daemon.font_field).small());

    let code_size_display = SharedString::from(format!("{:.0}", typo.code_font_size));
    let code_size_stepper = div()
        .flex()
        .items_center()
        .gap_1()
        .child(step_button(
            "code-size-minus",
            "−",
            &t,
            cx.listener(|_, _, _, cx| {
                settings::update(cx, |content| {
                    content.typography.code_font_size =
                        (content.typography.code_font_size - 1.0).max(10.0);
                });
            }),
        ))
        .child(div().min_w(px(44.)).text_xs().text_color(t.dim).child(code_size_display))
        .child(step_button(
            "code-size-plus",
            "+",
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
        .gap_1()
        .child(step_button(
            "line-height-minus",
            "−",
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
                .min_w(px(44.))
                .text_xs()
                .text_color(t.dim)
                .child(line_height_display),
        )
        .child(step_button(
            "line-height-plus",
            "+",
            &t,
            cx.listener(|_, _, _, cx| {
                settings::update(cx, |content| {
                    content.typography.prose_line_height =
                        (content.typography.prose_line_height + 0.05).min(2.0);
                });
            }),
        ));

    let widths = [640.0, 760.0, 850.0, 960.0];
    let next_width = widths
        .iter()
        .copied()
        .find(|w| *w > typo.content_width + 0.5)
        .unwrap_or(widths[0]);
    let width_cycle = div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().min_w(px(56.)).text_xs().text_color(t.dim).child(SharedString::from(format!("{:.0}px", typo.content_width))))
        .child(step_button(
            "width-cycle",
            "切换",
            &t,
            cx.listener(move |_, _, _, cx| {
                settings::update(cx, |content| content.typography.content_width = next_width);
            }),
        ));

    // —— 行为 ——
    let follow_buttons: Vec<AnyElement> = [("开", true), ("关", false)]
        .iter()
        .map(|(label, value)| {
            let value = *value;
            segmented_button(
                format!("follow-{label}"),
                *label,
                behavior.stream_follow == value,
                &t,
                cx.listener(move |_, _, _, cx| {
                    settings::update(cx, |content| content.behavior.stream_follow = value);
                }),
            )
            .into_any_element()
        })
        .collect();

    let tails = [(4096usize, "4KB"), (16 * 1024, "16KB"), (64 * 1024, "64KB")];
    let next_tail = tails
        .iter()
        .find(|(bytes, _)| *bytes > behavior.output_tail_max)
        .map(|(bytes, _)| *bytes)
        .unwrap_or(tails[0].0);
    let tail_display = tails
        .iter()
        .find(|(bytes, _)| *bytes == behavior.output_tail_max)
        .map(|(_, label)| *label)
        .unwrap_or("自定义");
    let tail_cycle = div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().text_xs().text_color(t.dim).child(tail_display))
        .child(step_button(
            "tail-cycle",
            "切换",
            &t,
            cx.listener(move |_, _, _, cx| {
                settings::update(cx, |content| content.behavior.output_tail_max = next_tail);
            }),
        ));

    // —— 布局 ——
    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(24.))
        .py_2()
        .border_b_1()
        .border_color(t.border)
        .bg(t.panel)
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("设置"))
        .child(action_button(cx, 
            "close-settings".into(),
            "关闭",
            t.dim,
            cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                daemon.settings_open = false;
                daemon.settings_form = None;
                cx.notify();
            }),
            &t,
        ));

    let footer = div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(24.))
        .py_2()
        .border_t_1()
        .border_color(t.border)
        .bg(t.panel)
        .child(
            div()
                .text_xs()
                .text_color(t.faint)
                .child(SharedString::from(format!(
                    "配置文件:{}",
                    settings::config_path().display()
                ))),
        )
        .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
            cx,
            qaqh_gpui_kit::kit::BtnKind::Danger,
            "reset-settings",
            "恢复默认",
            &t,
            cx.listener(|_, _, _, cx| {
                settings::update(cx, |content| {
                    *content = settings::SettingsContent::default();
                });
            }),
        )));

    let local = div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            section_card(
                "外观",
                vec![
                    setting_row(
                        "主题模式",
                        "跟随系统外观,或固定亮/暗色",
                        div().flex().gap_1().children(theme_buttons).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "密度",
                        "时间线留白节奏",
                        div().flex().gap_1().children(density_buttons).into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ),
        )
        .child(
            section_card(
                "排版",
                vec![
                    setting_row(
                        "界面字号",
                        "全局缩放基准(rem),影响全部界面文字",
                        font_size_stepper.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "界面字体",
                        "字体族名称,留空用平台默认;Enter 应用",
                        font_input.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "代码字号",
                        "代码块文字大小(px)",
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
                        "内容列宽",
                        "时间线正文列最大宽度",
                        width_cycle.into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ),
        )
        .child(
            section_card(
                "行为",
                vec![
                    setting_row(
                        "流式跟随",
                        "新内容自动滚到底部;滚轮上滚暂停,下滚恢复",
                        div().flex().gap_1().children(follow_buttons).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "输出尾窗上限",
                        "工具输出超过上限时仅显示尾部",
                        tail_cycle.into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ),
        );
    let is_local = daemon.settings_editor == SettingsEditor::Local;

    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .bg(t.bg)
        .child(header)
        .child(crate::settings_ui::navigation(daemon.settings_editor, cx, &t))
        .child(
            div()
                .id(format!("settings-scroll-{:?}-{}", daemon.settings_editor, daemon.settings_form.is_some()))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(24.))
                .py_4()
                .child(
                    div().max_w(px(840.)).mx_auto().child(if is_local {
                        local
                    } else {
                        crate::settings_ui::render(daemon, cx, &t)
                    }),
                ),
        )
        .when(is_local, |page| page.child(footer))
}

/// 新建会话面板从后端目录选择模型与模型支持的推理档位。
pub(crate) fn new_session_panel(
    daemon: &mut DaemonStore,
    cx: &mut Context<DaemonStore>,
) -> Div {
    let t = *cx.colors();
    let workspace_label = daemon
        .workspaces
        .get(daemon.new_workspace)
        .map(|workspace| workspace.title.clone())
        .unwrap_or_else(|| "无工作区".into());
    let levels = daemon.providers.as_deref().zip(daemon.new_model.as_ref())
        .map(|(catalog, model)| model_effort_levels(catalog, model)).unwrap_or_default();
    let effort_buttons = std::iter::once(None).chain(levels.iter().map(Some))
        .map(|level| {
            let active = daemon.new_model.as_ref()
                .and_then(|model| model.options.reasoning_effort.as_ref()) == level;
            let value = level.cloned();
            let label = level.map(String::as_str).unwrap_or("默认").to_owned();
            segmented_button(
                format!("ns-effort-{label}"), label, active, &t,
                cx.listener(move |daemon: &mut DaemonStore, _, _, cx| {
                    if let Some(model) = &mut daemon.new_model {
                        model.options.reasoning_effort = value.clone();
                        cx.notify();
                    }
                }),
            ).into_any_element()
        }).collect::<Vec<_>>();

    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(24.))
        .py_2()
        .border_b_1()
        .border_color(t.border)
        .bg(t.panel)
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("新建会话"))
        .child(action_button(cx, 
            "close-new-session".into(),
            "关闭",
            t.dim,
            cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                daemon.new_session_open = false;
                daemon.settings_form = None;
                cx.notify();
            }),
            &t,
        ));

    if let Some(form) = &daemon.settings_form {
        return div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(t.bg)
            .child(header)
            .child(
                div().id("new-model-picker").flex_1().min_h_0().overflow_y_scroll().p_4()
                    .child(crate::settings_ui::render_form(form, cx, &t)),
            );
    }

    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .bg(t.bg)
        .child(header)
        .child(
            div()
                .flex_1()
                .flex()
                .items_start()
                .justify_center()
                .p(px(24.))
                .child(
                    div()
                        .w(px(520.))
                        .mt(px(48.))
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            qaqh_gpui_kit::kit::card(&t)
                                .flex()
                                .flex_col()
                                .px_3()
                                .py_1()
                                .child(setting_row(
                                    "工作区",
                                    "会话的工作目录(cwd)",
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1()
                                        .child(
                                            div()
                                                .min_w(px(120.))
                                                .text_xs()
                                                .text_color(t.text)
                                                .child(SharedString::from(workspace_label)),
                                        )
                                        .child(step_button(
                                            "ns-workspace",
                                            "切换",
                                            &t,
                                            cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                                                if !daemon.workspaces.is_empty() {
                                                    daemon.new_workspace =
                                                        (daemon.new_workspace + 1) % daemon.workspaces.len();
                                                }
                                                cx.notify();
                                            }),
                                        ))
                                        .into_any_element(),
                                    &t,
                                ))
                                .child(setting_row(
                                    "服务商 / 模型",
                                    "创建会话需要有效的模型选择",
                                    div().flex().gap_2().items_center()
                                        .child(daemon.new_model.as_ref().map(|model| {
                                            format!("{} / {}", model.provider_id, model.model_id)
                                        }).unwrap_or_else(|| "跟随全局默认".into()))
                                        .child(step_button("ns-model", "选择", &t,
                                            cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                                                crate::settings_ui::choose_new_model(daemon, cx);
                                                cx.notify();
                                            }),
                                        )).into_any_element(),
                                    &t,
                                ))
                                .child(setting_row(
                                    "推理强度",
                                    "使用模型声明的推理档位",
                                    div().flex().gap_1().children(effort_buttons).into_any_element(),
                                    &t,
                                )),
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
                                        .child("不选模型 = 跟随全局默认;可在设置页修改默认模型"),
                                )
                                .child(gpui::AnyElement::from(qaqh_gpui_kit::kit::button(
                                    cx,
                                    qaqh_gpui_kit::kit::BtnKind::Primary,
                                    "create-session",
                                    if daemon.creating_session { "创建中…" } else { "创建会话" },
                                    &t,
                                    cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                                        daemon.create_session();
                                        cx.notify();
                                    }),
                                ))),
                        ),
                ),
        )
}

/// daemon 权限档位。
pub(crate) fn permission_row(daemon: &DaemonStore, cx: &mut Context<DaemonStore>, t: &Palette) -> Div {
    let control = match &daemon.config {
        Some(config) => {
            let buttons = [(1u8, "只读"), (2, "工作区写"), (3, "跳过授权"), (4, "沙箱运行")]
                .into_iter()
                .map(|(level, label)| {
                    segmented_button(
                        format!("perm-{level}"),
                        label,
                        config.permission_level == level,
                        t,
                        cx.listener(move |daemon: &mut DaemonStore, _, _, _| {
                            let _ = daemon.requests.unbounded_send(BridgeRequest::SaveConfig(ConfigPatch {
                                permission_level: Some(u64::from(level)),
                                ..Default::default()
                            }));
                        }),
                    )
                });
            div().flex().gap_1().children(buttons)
        }
        None => div().text_xs().text_color(t.dim).child("读取中…"),
    };
    setting_row("权限档位", "daemon 全局权限策略", control.into_any_element(), t)
}
