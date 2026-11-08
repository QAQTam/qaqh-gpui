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
use gpui_component::{Sizable, Size, WindowExt};
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_ringing_contract::fact::AssistantBlockKind;
use qaqh_ringing_contract::fact::ContentValue;
use qaqh_ringing_contract::fact::InteractionKind;
use qaqh_ringing_contract::fact::ToolCallId;
use qaqh_ringing_contract::state::ExecSessionState;
use qaqh_ringing_contract::state::TimelineEntry;
use qaqh_ringing_contract::state::ToolCallState;
use qaqh_ringing_contract::state::ToolCallView;
use qaqh_types::ToolCallOutcome;
use qaqh_client::SessionSettingsPatch;
use qaqh_config_api::ConfigPatch;

use crate::ask;
use crate::bridge::BridgeRequest;
use crate::settings;
use crate::settings::Density;
use crate::store::DaemonStore;
use crate::store::SessionRow;
use crate::store::{SessionStore, SettingsEditor};
use crate::store::StreamState;
use crate::theme::ActiveTheme;
use crate::theme::Palette;
use crate::theme::ThemeMode;

pub(crate) fn content_text(value: &ContentValue) -> &str {
    match value {
        ContentValue::Inline { text } => text,
        ContentValue::Ref { .. } => "引用内容：点击上方「加载引用内容」读取",
        ContentValue::Unavailable(_) => "内容不可用",
    }
}

fn state_label(state: &ToolCallState) -> &'static str {
    match state {
        ToolCallState::Declared => "已声明",
        ToolCallState::AwaitingInteraction { .. } => "等待授权",
        ToolCallState::Running => "执行中",
        ToolCallState::Cancelling => "取消中",
        ToolCallState::Finished { outcome } => match outcome {
            ToolCallOutcome::Completed => "完成",
            ToolCallOutcome::Yielded { .. } => "后台运行",
            ToolCallOutcome::Failed { .. } => "失败",
            ToolCallOutcome::Cancelled { .. } => "已取消",
            ToolCallOutcome::Interrupted { .. } => "已中断",
        },
    }
}

fn state_color(state: &ToolCallState, t: &Palette) -> Hsla {
    match state {
        ToolCallState::Declared => t.dim,
        ToolCallState::AwaitingInteraction { .. } => t.warn,
        ToolCallState::Running => t.accent,
        ToolCallState::Cancelling => t.warn,
        ToolCallState::Finished { outcome } => match outcome {
            ToolCallOutcome::Completed => t.ok,
            ToolCallOutcome::Yielded { .. } => t.accent,
            ToolCallOutcome::Failed { .. } => t.bad,
            ToolCallOutcome::Cancelled { .. } | ToolCallOutcome::Interrupted { .. } => t.dim,
        },
    }
}

fn exec_state_label(state: &ExecSessionState, t: &Palette) -> (&'static str, Hsla) {
    match state {
        ExecSessionState::Running => ("运行中", t.accent),
        ExecSessionState::Stopping { .. } => ("停止中", t.warn),
        ExecSessionState::Finished { .. } => ("已结束", t.dim),
    }
}

fn activity_label(activity: ActivityState) -> &'static str {
    match activity {
        ActivityState::Idle => "空闲",
        ActivityState::Running => "运行中",
        ActivityState::WaitingUser => "等待输入",
        ActivityState::Cancelling => "取消中",
    }
}

/// 侧栏状态点:running/waiting_user/cancelling 显示,其余不显示。
fn status_dot(status: &str, t: &Palette) -> Div {
    let (color, label) = match status {
        "running" => (t.accent, "运行中"),
        "waiting_user" => (t.warn, "待响应"),
        "cancelling" => (t.warn, "取消中"),
        "idle" => (t.dim, "空闲"),
        _ => (t.dim, ""),
    };
    if label.is_empty() {
        return div();
    }
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(
            div()
                .size_1p5()
                .rounded_full()
                .bg(color),
        )
        .child(div().text_xs().text_color(color).child(label))
}

fn stream_badge(state: &StreamState, t: &Palette) -> Div {
    let (color, label): (Hsla, SharedString) = match state {
        StreamState::Connecting => (t.warn, "连接中".into()),
        StreamState::Open => (t.ok, "实时".into()),
        StreamState::Reconnecting { retry_ms } => {
            (t.warn, SharedString::from(format!("重连 {retry_ms}ms")))
        }
        StreamState::Closed { reason } => (t.bad, SharedString::from(format!("断开:{reason}"))),
    };
    div()
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_xs()
        .bg(t.btn)
        .text_color(color)
        .child(label)
}

/// 侧栏条目:选中 = accent 左边条 + selected 底色(借鉴 zed project_panel
/// 的「背景 + 侧边状态边」模式,不搞圆角高亮)。
pub(crate) fn sidebar_row(
    row: &SessionRow,
    selected: bool,
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
                .child(status_dot(&row.status, t))
                .child(
                    div()
                        .text_sm()
                        .text_color(if selected { t.accent } else { t.text })
                        .child(title),
                ),
        )
        .child(div().text_xs().text_color(t.dim).child(row.updated_at.clone()))
}


/// 右栏任务 dock:当前会话的 todo 快照。无数据/无任务时不渲染,不占正文宽度。
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
    let mut dock = div()
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
                .text_xs()
                .text_color(t.dim)
                .border_b_1()
                .border_color(t.border)
                .child("任务"),
        );
    let done = items
        .iter()
        .filter(|item| item.status == "completed")
        .count();
    dock = dock.child(
        div()
            .px_3()
            .py_1()
            .text_xs()
            .text_color(t.ok)
            .child(SharedString::from(format!("{done}/{} 完成", items.len()))),
    );
    let mut list = div()
        .id("todo-list")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .flex()
        .flex_col();
    for item in items {
        let (mark, color) = match item.status.as_str() {
            "in_progress" => ("◐", t.accent),
            "completed" => ("●", t.ok),
            "cancelled" => ("✕", t.bad),
            _ => ("○", t.dim),
        };
        list = list.child(
            div()
                .px_3()
                .py_1p5()
                .flex()
                .gap_2()
                .child(div().text_sm().text_color(color).child(mark))
                .child(
                    div()
                        .text_sm()
                        .text_color(if item.status == "completed" || item.status == "cancelled" {
                            t.dim
                        } else {
                            t.text
                        })
                        .child(item.title.clone()),
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
        .child(action_button(cx, 
            "reset-settings".into(),
            "恢复默认",
            t.bad,
            cx.listener(|_, _, _, cx| {
                settings::update(cx, |content| {
                    *content = settings::SettingsContent::default();
                });
            }),
            &t,
        ));

    let local = div()
        .flex()
        .flex_col()
        .child(section_header("外观", &t))
        .child(setting_row(
            "主题模式",
            "跟随系统外观,或固定亮/暗色",
            div().flex().gap_1().children(theme_buttons).into_any_element(),
            &t,
        ))
        .child(setting_row(
            "密度",
            "时间线留白节奏",
            div().flex().gap_1().children(density_buttons).into_any_element(),
            &t,
        ))
        .child(section_header("排版", &t))
        .child(setting_row(
            "界面字号",
            "全局缩放基准(rem),影响全部界面文字",
            font_size_stepper.into_any_element(),
            &t,
        ))
        .child(setting_row(
            "界面字体",
            "字体族名称,留空用平台默认;Enter 应用",
            font_input.into_any_element(),
            &t,
        ))
        .child(setting_row(
            "代码字号",
            "代码块文字大小(px)",
            code_size_stepper.into_any_element(),
            &t,
        ))
        .child(setting_row(
            "正文行高",
            "消息段落行高倍数",
            line_height_stepper.into_any_element(),
            &t,
        ))
        .child(setting_row(
            "内容列宽",
            "时间线正文列最大宽度",
            width_cycle.into_any_element(),
            &t,
        ))
        .child(section_header("行为", &t))
        .child(setting_row(
            "流式跟随",
            "新内容自动滚到底部;滚轮上滚暂停,下滚恢复",
            div().flex().gap_1().children(follow_buttons).into_any_element(),
            &t,
        ))
        .child(setting_row(
            "输出尾窗上限",
            "工具输出超过上限时仅显示尾部",
            tail_cycle.into_any_element(),
            &t,
        ));
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
                .py_3()
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

fn next_option<'a, T: PartialEq>(options: &'a [T], current: Option<&T>) -> Option<&'a T> {
    let index = current.and_then(|current| options.iter().position(|option| option == current));
    match index {
        Some(index) => options.get(index + 1).or_else(|| options.first()),
        None => options.first(),
    }
}

fn next_model<'a>(
    options: &'a [qaqh_types::ModelSelection],
    current: Option<&qaqh_types::ModelSelection>,
) -> Option<&'a qaqh_types::ModelSelection> {
    let current = current.and_then(|current| options.iter().find(|model| {
        model.provider_id == current.provider_id && model.model_id == current.model_id
    }));
    next_option(options, current)
}

fn model_effort_levels(catalog: &qaqh_profile::ProfilesDocument, model: &qaqh_types::ModelSelection) -> Vec<String> {
    catalog.providers.get(&model.provider_id)
        .and_then(|provider| provider.models.get(&model.model_id))
        .filter(|model| model.capabilities.reasoning)
        .map(|model| model.reasoning_levels.clone()).unwrap_or_default()
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
                        .w(px(480.))
                        .mt(px(48.))
                        .flex()
                        .flex_col()
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
                                }).unwrap_or_else(|| "没有可用模型".into()))
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
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .pt_2()
                                .child(action_button(cx, 
                                    "create-session".into(),
                                    if daemon.creating_session { "创建中…" }
                                    else if daemon.new_model.is_some() { "创建" }
                                    else { "缺少模型" },
                                    if daemon.new_model.is_some() && !daemon.creating_session { t.ok } else { t.dim },
                                    cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                                        daemon.create_session();
                                        cx.notify();
                                    }),
                                    &t,
                                )),
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

pub(crate) fn section_header(title: &str, t: &Palette) -> Div {
    div()
        .pt_4()
        .pb_1()
        .text_xs()
        .text_color(t.dim)
        .child(SharedString::from(title))
}

/// 设置行(zed settings_ui 行布局):左标题+描述,右控件,行间分隔线。
pub(crate) fn setting_row(label: &str, desc: &str, control: AnyElement, t: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .py_2()
        .border_b_1()
        .border_color(t.border)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .max_w(px(420.))
                .child(div().text_sm().text_color(t.text).child(SharedString::from(label)))
                .child(div().text_xs().text_color(t.dim).child(SharedString::from(desc))),
        )
        .child(control)
}

pub(crate) fn segmented_button(
    id: String,
    label: impl Into<SharedString>,
    active: bool,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .px_2()
        .py_1()
        .rounded_sm()
        .text_xs()
        .cursor_pointer()
        .bg(if active { t.accent_bg } else { t.btn })
        .text_color(if active { t.accent } else { t.dim })
        .hover(|style| style.bg(t.btn_hover))
        .on_click(listener)
        .child(label.into())
}

fn step_button(
    id: &'static str,
    glyph: &'static str,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_sm()
        .text_xs()
        .cursor_pointer()
        .min_w(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .bg(t.btn)
        .text_color(t.text)
        .hover(|style| style.bg(t.btn_hover))
        .on_click(listener)
        .child(glyph)
}

fn user_bubble(text: &str, t: &Palette) -> Div {
    div()
        .self_end()
        .max_w(px(640.))
        .px_3()
        .py_2()
        .rounded_lg()
        .bg(t.bubble)
        .text_sm()
        .child(text.to_string())
}

/// UTF-8 安全尾部截断(参照 electron 的 16KB 尾策略,不劈多字节字)。
fn utf8_tail(text: &str, max_bytes: usize) -> (&str, bool) {
    if text.len() <= max_bytes {
        return (text, false);
    }
    let start = text.ceil_char_boundary(text.len() - max_bytes);
    (text.split_at(start).1, true)
}

fn tool_summary(name: &str, state: &ToolCallState) -> Option<&'static str> {
    let (active, done) = match name {
        "todo_write" => ("正在创建待办", "已创建待办"),
        "todo_update" => ("正在更新待办", "已更新待办"),
        "todo_list" => ("正在查看待办", "已查看待办"),
        "ask" => ("正在询问用户", "用户已回答"),
        "read" => ("正在读取文件", "已读取文件"),
        "read_image" => ("正在查看图片", "已查看图片"),
        "glob" => ("正在查找文件", "已查找文件"),
        "grep" => ("正在搜索内容", "已搜索内容"),
        "edit" | "apply_patch" => ("正在编辑文件", "已编辑文件"),
        "write" => ("正在写入文件", "已写入文件"),
        "delete" => ("正在删除文件", "已删除文件"),
        "search" => ("正在搜索网页", "已搜索网页"),
        "web_fetch" => ("正在读取网页", "已读取网页"),
        _ => return None,
    };
    match state {
        ToolCallState::Finished { outcome } => outcome.is_success().then_some(done),
        ToolCallState::Declared | ToolCallState::Running => Some(active),
        ToolCallState::AwaitingInteraction { .. } if name == "ask" => Some(active),
        _ => None,
    }
}

fn tool_status_only(name: &str) -> bool {
    matches!(name, "ask" | "todo_write" | "todo_update" | "todo_list")
}

fn tool_status_line(title: String, t: &Palette) -> Div {
    div().px_2().py_1().text_xs().text_color(t.dim).truncate()
        .child(SharedString::from(title))
}

fn tool_toggle(
    entity: &Entity<SessionStore>,
    call_id: &ToolCallId,
    expanded: bool,
    title: String,
    t: &Palette,
) -> gpui::Stateful<Div> {
    let entity = entity.downgrade();
    let call_id = call_id.clone();
    div()
        .id(SharedString::from(format!("tool-toggle-{}", call_id.0)))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .text_xs()
        .text_color(t.dim)
        .cursor_pointer()
        .hover(|style| style.bg(t.btn_hover))
        .child(div().flex_shrink_0().child(if expanded { "▾" } else { "▸" }))
        .child(div().min_w_0().truncate().child(SharedString::from(title)))
        .on_click(move |_, _, cx| {
            let _ = entity.update(cx, |store, cx| {
                if !store.expanded_tools.remove(&call_id) {
                    store.expanded_tools.insert(call_id.clone());
                }
                cx.notify();
            });
        })
}

fn tool_card(
    cx: &gpui::App,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    tool: &ToolCallView,
    t: &Palette,
    tail_max: usize,
) -> Div {
    let name = tool.tool_name.as_deref().unwrap_or("(未知工具)");
    let exec = store.exec_sessions.iter().find(|exec| exec.origin_call_id == tool.call_id);
    let is_exec = exec.is_some() || name == "exec_command";
    let command = if let Some(exec) = exec {
        Some(exec.command.as_str())
    } else if is_exec {
        tool.args.as_ref().and_then(|args| args.get("cmd")?.as_str())
    } else {
        None
    };
    let (status, status_color) = if let Some(exec) = exec {
        exec_state_label(&exec.state, t)
    } else {
        (state_label(&tool.state), state_color(&tool.state, t))
    };
    let title = if let Some(command) = command {
        let command = command.replace(['\r', '\n'], " ");
        format!("{name}  $ {command}  ·  {status}")
    } else if let Some(summary) = tool_summary(name, &tool.state) {
        summary.to_owned()
    } else {
        format!("{name}  ·  {status}")
    };
    if tool_status_only(name) {
        return tool_status_line(title, t);
    }
    let expanded = store.expanded_tools.contains(&tool.call_id);
    let header = tool_toggle(entity, &tool.call_id, expanded, title, t);
    if !expanded {
        return div().child(header);
    }
    let mut card = div()
        .flex()
        .flex_col()
        .gap_1()
        .max_w_full()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(t.border)
        .bg(if is_exec { t.card } else { t.panel })
        .child(header)
        .child(
            div()
                .flex()
                .gap_2()
                .items_center()
                .child(
                    div()
                        .text_sm()
                        .flex_shrink_0()
                        .text_color(t.accent)
                        .child(SharedString::from(format!("⚙ {name}"))),
                )
                .children(command.map(|command| {
                    div().min_w_0().text_sm().text_color(t.text)
                        .child(SharedString::from(format!("$ {command}")))
                }))
                .child(
                    div()
                        .text_xs()
                        .flex_shrink_0()
                        .text_color(status_color)
                        .child(status),
                ),
        );
    if !is_exec && let Some(args) = &tool.args {
        card = card.child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child(SharedString::from(format!("参数 {args}"))),
        );
    }
    let raw_output = if let Some(exec) = exec {
        store.exec_output_tail.iter()
            .find(|(id, end, text)| id == &exec.id && *end > exec.next_offset && !text.is_empty())
            .map_or(exec.output_preview.as_str(), |(_, _, text)| text.as_str())
    } else if matches!(tool.state, ToolCallState::Finished { .. }) {
        tool.output.as_ref().map_or("", content_text)
    } else {
        store
            .output_tail
            .iter()
            .find(|(call_id, _)| call_id == &tool.call_id)
            .map_or("", |(_, tail)| tail.as_str())
    };
    // 历史快照可能只保留工具结果，提取命令输出而非展示结果 JSON。
    let result = if is_exec && exec.is_none() {
        serde_json::from_str::<serde_json::Value>(raw_output).ok()
    } else {
        None
    };
    let output = result.as_ref()
        .and_then(|result| result.get("output")?.as_str())
        .unwrap_or(raw_output);
    let (output, truncated) = utf8_tail(output, tail_max);
    if !output.is_empty() {
        card = card.child(
            div()
                .text_xs()
                .text_color(t.dim)
                .child(SharedString::from(if truncated {
                    format!("…{output}")
                } else {
                    output.to_string()
                })),
        );
        if truncated {
            card = card.child(div().text_xs().text_color(t.faint).child("(仅显示输出尾部)"));
        }
    }
    if let Some(exec) = exec && !exec.state.is_terminal() {
        let id = exec.id.0.clone();
        let control = |terminate| {
            let entity = entity.downgrade();
            let exec_id = id.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut gpui::App| {
                let _ = entity.update(cx, |store, _| store.exec_control(exec_id.clone(), terminate));
            }
        };
        card = card.child(
            div()
                .flex()
                .gap_2()
                .child(action_button(cx, format!("exec-int-{id}"), "中断", t.warn, control(false), t))
                .child(action_button(cx, format!("exec-term-{id}"), "终止", t.bad, control(true), t)),
        );
    }
    card
}

fn has_assistant_text(kind: AssistantBlockKind, text: &str) -> bool {
    kind != AssistantBlockKind::ToolCall && !text.trim().is_empty()
}

fn hidden_timeline_entry(entry: &TimelineEntry) -> bool {
    match entry {
        TimelineEntry::ToolResult { .. } => true,
        TimelineEntry::AssistantBlock { block_kind, content, .. } => {
            !has_assistant_text(*block_kind, content_text(content))
        }
        _ => false,
    }
}

/// 助手块 markdown 区。流式光标单独渲染，避免改变 Markdown 源。
fn assistant_section(
    kind: &AssistantBlockKind,
    text: &str,
    live: bool,
    section: usize,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    t: &Palette,
    ty: &settings::Typography,
) -> gpui::Stateful<Div> {
    let label = match kind {
        AssistantBlockKind::Reasoning => "思考",
        AssistantBlockKind::Answer => "回答",
        AssistantBlockKind::ToolCall => "工具",
    };
    let blocks = store.md_cache.get(section, text);
    div()
        .id(("mdsec", section))
        .flex()
        .flex_col()
        .gap_1()
        .max_w_full()
        .child(div().text_xs().text_color(t.dim).child(label))
        .children(crate::markdown::render_blocks(&blocks, section, t, ty, entity))
        .children(live.then(|| div().text_xs().text_color(t.dim).child("▍")))
}

fn timeline_entry(
    cx: &gpui::App,
    store: &SessionStore,
    entity: &Entity<SessionStore>,
    entry: &TimelineEntry,
    index: usize,
    t: &Palette,
    ty: &settings::Typography,
    tail_max: usize,
) -> Option<AnyElement> {
    Some(match entry {
        TimelineEntry::Input { content, images, attachments, .. } => div()
            .child(user_bubble(content_text(content), t))
            .child(div().flex().flex_wrap().gap_2().children(images.iter().map(crate::media::preview)))
            .children((!attachments.is_empty()).then(|| div().text_xs().child(format!("{} 个文本附件", attachments.len()))))
            .into_any_element(),
        TimelineEntry::AssistantBlock { block_kind, content, .. } => {
            assistant_section(block_kind, content_text(content), false, index * 2, store, entity, t, ty)
                .into_any_element()
        }
        TimelineEntry::ToolCall { call_id, .. } => {
            let tool = store.tools.iter().find(|tool| &tool.call_id == call_id)?;
            tool_card(cx, store, entity, tool, t, tail_max).into_any_element()
        }
        TimelineEntry::Compaction { .. } => div()
            .text_xs()
            .text_color(t.dim)
            .child("── 已压缩 ──")
            .into_any_element(),
        TimelineEntry::ToolResult { .. } => return None,
    })
}

pub(crate) fn action_button(
    cx: &gpui::App,
    id: String,
    label: &'static str,
    color: Hsla,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    t: &Palette,
) -> impl IntoElement {
    // gpui-component Button:保留"中性底 + 彩色字"的既有视觉语义,
    // 换取统一的焦点环/按压态/禁用态;签名新增 cx,调用面已 rg 验证同步。
    Button::new(SharedString::from(id))
        .with_size(Size::XSmall)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(t.btn)
                .foreground(color)
                .hover(t.btn_hover)
                .active(t.btn_hover),
        )
        .label(label)
        .on_click(listener)
}

fn interaction_card(
    kind_label: &str,
    request: &str,
    buttons: Vec<AnyElement>,
    body: Vec<AnyElement>,
    t: &Palette,
) -> Div {
    let mut card = div()
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .rounded_md()
        .border_1()
        .border_color(t.warn)
        .bg(t.warn_bg);
    card = card.child(
        div()
            .text_sm()
            .text_color(t.warn)
            .child(SharedString::from(format!("待交互 · {kind_label}"))),
    );
    card = card.child(div().text_sm().child(SharedString::from(request.to_string())));
    if !body.is_empty() {
        card = card.child(div().flex().flex_col().gap_1().children(body));
    }
    if !buttons.is_empty() {
        card = card.child(div().flex().gap_2().children(buttons));
    }
    card
}

impl SessionStore {
    /// ask 应答 Dialog 内容:解析 request 渲染问题/选项/自定义输入;
    /// 解析失败回退自由文本。提交/跳过成功后收口对话框。
    fn ask_dialog(
        &mut self,
        interaction: &qaqh_ringing_contract::state::InteractionView,
        dialog: Dialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Dialog {
        let t = *cx.colors();
        let iid = interaction.interaction_id.clone();
        let questions = ask::parse_request(&interaction.request);
        match questions {
            Ok(questions) if !questions.is_empty() => {
                let ids: Vec<String> = questions.iter().map(|q| q.id.clone()).collect();
                let mut body: Vec<AnyElement> = Vec::new();
                if let Some(hint) = &self.ask_hint {
                    body.push(
                        div()
                            .text_xs()
                            .text_color(t.bad)
                            .child(SharedString::from(hint.clone()))
                            .into_any_element(),
                    );
                }
                for (index, question) in questions.iter().enumerate() {
                    let field = self.ask_draft(&iid, questions.len(), window, cx).customs[index].clone();
                    let mut row = div().flex().flex_col().gap_1();
                    row = row.child(
                        div()
                            .text_sm()
                            .child(SharedString::from(format!(
                                "{}. {}",
                                index + 1,
                                question.question
                            ))),
                    );
                    for option in &question.options {
                        let chosen = self.ask_draft(&iid, questions.len(), window, cx).answers[index]
                            .as_deref()
                            == Some(option.as_str());
                        let opt = option.clone();
                        let opt_click = opt.clone();
                        let iid_click = iid.clone();
                        row = row.child(
                            div()
                                .id(SharedString::from(format!("ask-{iid}-{index}-{opt}")))
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .cursor_pointer()
                                .border_1()
                                .border_color(if chosen { t.accent } else { t.border })
                                .bg(if chosen { t.accent_bg } else { t.btn })
                                .text_color(if chosen { t.accent } else { t.text })
                                .hover(|style| style.bg(t.btn_hover))
                                .on_click(cx.listener(
                                    move |store: &mut SessionStore, _, window: &mut Window, cx| {
                                        let draft = store.ask_draft(&iid_click, index + 1, window, cx);
                                        draft.answers[index] = Some(opt_click.clone());
                                        draft.customs[index].update(cx, |custom, cx| custom.set_value("", window, cx));
                                        cx.notify();
                                    },
                                ))
                                .child(SharedString::from(format!(
                                    "{} {opt}",
                                    if chosen { "●" } else { "○" }
                                ))),
                        );
                    }
                    if question.allow_custom {
                        row = row.child(Input::new(&field).small().w_full());
                    }
                    body.push(row.into_any_element());
                }
                let submit = {
                    let iid_submit = iid.clone();
                    let ids = ids.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        if store.ask_submit(&iid_submit, &ids, cx).is_some() {
                            store.ask_hint = None;
                            window.close_dialog(cx);
                        } else {
                            store.ask_hint = Some("请先选择答案或填写自定义回答".into());
                        }
                        cx.notify();
                    })
                };
                let skip = {
                    let iid_skip = iid.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        store.ask_dismiss(&iid_skip);
                        store.ask_hint = None;
                        window.close_dialog(cx);
                        cx.notify();
                    })
                };
                dialog
                    .title("会话提问")
                    .children(body)
                    .footer(
                        div().flex().justify_end().gap_2()
                            .child(action_button(cx, format!("ask-submit-{iid}"), "提交回答", t.ok, submit, &t))
                            .child(action_button(cx, format!("ask-skip-{iid}"), "跳过", t.dim, skip, &t)),
                    )
            }
            _ => {
                // 解析失败:回退自由文本(q1),不猜语法。
                let field = self.ask_draft(&iid, 1, window, cx).customs[0].clone();
                let submit = {
                    let ids = vec!["q1".to_string()];
                    let iid_submit = iid.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        if store.ask_submit(&iid_submit, &ids, cx).is_some() {
                            store.ask_hint = None;
                            window.close_dialog(cx);
                        } else {
                            store.ask_hint = Some("请填写回答".into());
                        }
                        cx.notify();
                    })
                };
                let skip = {
                    let iid_skip = iid.clone();
                    cx.listener(move |store: &mut SessionStore, _, window: &mut Window, cx| {
                        store.ask_dismiss(&iid_skip);
                        store.ask_hint = None;
                        window.close_dialog(cx);
                        cx.notify();
                    })
                };
                let mut body: Vec<AnyElement> = Vec::new();
                body.push(
                    div()
                        .text_sm()
                        .child(SharedString::from(content_text(&interaction.request).to_owned()))
                        .into_any_element(),
                );
                if let Some(hint) = &self.ask_hint {
                    body.push(
                        div()
                            .text_xs()
                            .text_color(t.bad)
                            .child(SharedString::from(hint.clone()))
                            .into_any_element(),
                    );
                }
                body.push(Input::new(&field).small().w_full().into_any_element());
                dialog
                    .title("会话提问")
                    .children(body)
                    .footer(
                        div().flex().justify_end().gap_2()
                            .child(action_button(cx, format!("ask-submit-{iid}"), "提交回答", t.ok, submit, &t))
                            .child(action_button(cx, format!("ask-skip-{iid}"), "跳过", t.dim, skip, &t)),
                    )
            }
        }
    }
}

/// 打开 ask 应答 Dialog(模态覆盖层,Root 承载):到达即弹、footer 可重开;
/// 提问已处理时显示收口态。
pub(crate) fn open_ask_dialog(
    store: &Entity<SessionStore>,
    interaction_id: qaqh_ringing_contract::fact::InteractionId,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    window.open_dialog(cx, {
        let store = store.clone();
        move |dialog, window, cx| {
            store.update(cx, |store, cx| {
                let t = *cx.colors();
                match store
                    .interactions
                    .iter()
                    .find(|view| view.interaction_id == interaction_id)
                {
                    Some(interaction) if interaction.resolution.is_none() => {
                        let interaction = interaction.clone();
                        store.ask_dialog(&interaction, dialog, window, cx)
                    }
                    _ => dialog
                        .title("会话提问")
                        .child(div().text_sm().text_color(t.dim).child("该提问已处理。"))
                        .footer(
                            div().flex().justify_end().child(action_button(
                                cx,
                                "ask-dialog-done".into(),
                                "关闭",
                                t.dim,
                                |_, window: &mut Window, cx: &mut gpui::App| window.close_dialog(cx),
                                &t,
                            )),
                        ),
                }
            })
        }
    });
}

impl Render for SessionStore {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = *cx.colors();
        let title = self.title.clone().unwrap_or_else(|| self.session_id.clone());

        // 头部:标题(或重命名输入)+ 流徽标 + 重命名/中断按钮。
        let mut header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(t.border)
            .bg(t.panel);
        if let Some(draft) = self.renaming.clone() {
            // Enter 提交由 begin_rename 的事件订阅驱动;Escape 关闭重命名
            // (在祖先层截获,先于输入引擎的 Escape 动作)。
            let rename_input = div()
                .id("rename-input")
                .min_w(px(200.))
                .on_key_down(cx.listener(
                    |store: &mut SessionStore, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            store.rename_cancel();
                            cx.stop_propagation();
                            cx.notify();
                        }
                    },
                ))
                .child(Input::new(&draft).small());
            header = header
                .child(rename_input)
                .child(
                    action_button(cx, 
                        "rename-commit".into(),
                        "✓",
                        t.ok,
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.rename_commit(cx);
                            cx.notify();
                        }),
                        &t,
                    ),
                )
                .child(
                    action_button(cx, 
                        "rename-cancel".into(),
                        "✕",
                        t.dim,
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.rename_cancel();
                            cx.notify();
                        }),
                        &t,
                    ),
                );
        } else {
            header = header
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child(title),
                )
                .child(stream_badge(&self.stream, &t))
                .child(
                    div()
                        .id("model-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .bg(t.btn)
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.btn_hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            let current = store.settings.as_ref().and_then(|s| s.meta.model_selection.as_ref());
                            if let Some(catalog) = &store.catalog {
                                let options = crate::store::model_options_from(catalog);
                                if let Some(next) = next_model(&options, current).cloned() {
                                    store.patch_model(next);
                                    cx.notify();
                                }
                            }
                        }))
                        .child(format!(
                            "模型:{}",
                            self.settings.as_ref().and_then(|s| s.meta.model_selection.as_ref())
                                .map(|model| format!("{}/{}", model.provider_id, model.model_id))
                                .unwrap_or_else(|| "未选择".into()),
                        )),
                )
                .child(
                    div()
                        .id("effort-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .bg(t.btn)
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.btn_hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            let model = store.settings.as_ref().and_then(|settings| settings.meta.model_selection.as_ref());
                            if let Some((catalog, model)) = store.catalog.as_deref().zip(model) {
                                let levels = model_effort_levels(catalog, model);
                                let current = model.options.reasoning_effort.as_ref();
                                let next = match current {
                                    Some(current) => levels.iter().position(|level| level == current)
                                        .and_then(|index| levels.get(index + 1)),
                                    None => levels.first(),
                                };
                                store.patch_effort(next.cloned());
                                cx.notify();
                            }
                        }))
                        .child(format!("强度:{}", self.settings.as_ref()
                            .and_then(|s| s.meta.model_selection.as_ref())
                            .and_then(|model| model.options.reasoning_effort.as_deref())
                            .unwrap_or("默认"))),
                )
                .child(
                    div()
                        .id("tools-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .bg(t.btn)
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.btn_hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            let current = store.settings.as_ref().map(|s| s.meta.tool_mode.as_str());
                            let presets = [qaqh_types::STANDARD, qaqh_types::MINIMAL, qaqh_types::MINIMAL_B, qaqh_types::MINIMAL_C];
                            if let Some(next) = next_option(&presets, current.as_ref()) {
                                store.patch_tools_mode((*next).to_string());
                                cx.notify();
                            }
                        }))
                        .child(format!("工具:{}", self.settings.as_ref()
                            .map(|s| s.meta.tool_mode.as_str()).filter(|mode| !mode.is_empty())
                            .unwrap_or(qaqh_types::STANDARD))),
                )
                .child(action_button(cx, 
                    "session-mode".into(),
                    if self.settings.as_ref().is_some_and(|s| s.meta.mode == 1) { "Plan" } else { "Code" },
                    t.dim,
                    cx.listener(|store: &mut SessionStore, _, _, cx| {
                        let mode = if store.settings.as_ref().is_some_and(|s| s.meta.mode == 1) { "code" } else { "plan" };
                        store.patch_settings(SessionSettingsPatch { mode: Some(mode.into()), ..Default::default() });
                        cx.notify();
                    }),
                    &t,
                ))
                .child(
                    div()
                        .id("detach-chip")
                        .px_2()
                        .py_0p5()
                        .rounded_md()
                        .text_xs()
                        .cursor_pointer()
                        .bg(t.btn)
                        .text_color(t.dim)
                        .hover(|style| style.bg(t.btn_hover))
                        .on_click(cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.detach_from_workspace();
                            cx.notify();
                        }))
                        .child("脱离"),
                )
                .child(
                    action_button(cx, 
                        format!("rename-{}", self.session_id),
                        "重命名",
                        t.dim,
                        cx.listener(|store: &mut SessionStore, _, window, cx| {
                            store.begin_rename(window, cx);
                            cx.notify();
                        }),
                        &t,
                    ),
                )
                .child(
                    action_button(cx, 
                        format!("delete-{}", self.session_id),
                        if self.confirm_delete { "确认删除" } else { "删除" },
                        if self.confirm_delete { t.bad } else { t.dim },
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            if store.confirm_delete {
                                store.delete_self();
                            }
                            store.confirm_delete = !store.confirm_delete;
                            cx.notify();
                        }),
                        &t,
                    ),
                );
        }
        if self.activity == Some(ActivityState::Running) {
            header = header.child(
                div().flex_1().child(div()).child(
                    action_button(cx, 
                        format!("cancel-{}", self.session_id),
                        "中断",
                        t.bad,
                        cx.listener(|store: &mut SessionStore, _, _, cx| {
                            store.cancel_turn();
                            // 看门狗:5s 无终态则本地收口 + 重灌快照。
                            store.arm_cancel_watchdog(5000);
                            cx.notify();
                        }),
                        &t,
                    ),
                ),
            );
        } else if let Some(activity) = self.activity {
            header = header.child(
                div()
                    .flex_1()
                    .child(div())
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(activity_label(activity)),
                    ),
            );
        }

        let settings = settings::get(cx).clone();
        let ty = settings.typography();
        let tail_max = settings.behavior.output_tail_max;
        self.output_budget = tail_max;

        let list_state = self.timeline_list.clone();
        let old_history_len = list_state.item_count() - 1;
        if old_history_len != self.entries.len() {
            let start = old_history_len.min(self.entries.len());
            list_state.splice(start..old_history_len, self.entries.len() - start);
        }
        let follow = settings.behavior.stream_follow;
        if self.timeline_follow_setting != Some(follow) {
            list_state.set_follow_mode(if follow { FollowMode::Tail } else { FollowMode::Normal });
            self.timeline_follow_setting = Some(follow);
        }
        let entity = cx.entity();
        let timeline = list(list_state, move |index, _, cx| {
            let store = entity.read(cx);
            let row = div().w_full();
            let gap = px((8.0 + ty.density_shift).max(2.));
            if let Some(entry) = store.entries.get(index) {
                if hidden_timeline_entry(entry) {
                    return row.into_any_element();
                }
                let adjacent_tools = matches!(entry, TimelineEntry::ToolCall { .. })
                    && matches!(
                        store.entries.iter().skip(index + 1)
                            .find(|entry| !hidden_timeline_entry(entry)),
                        Some(TimelineEntry::ToolCall { .. })
                    );
                return match timeline_entry(cx, store, &entity, entry, index, &t, &ty, tail_max) {
                    Some(content) => row.pb(if adjacent_tools { px(0.) } else { gap })
                        .child(content).into_any_element(),
                    None => row.into_any_element(),
                };
            }

            let referenced_tools: HashSet<_> = store.entries.iter().filter_map(|entry| {
                match entry {
                    TimelineEntry::ToolCall { call_id, .. } => Some(call_id),
                    _ => None,
                }
            }).collect();
            let footer = row.flex().flex_col().gap(gap)
                .children(
                    store.streaming.iter().enumerate()
                        .filter(|(_, (_, kind, text))| has_assistant_text(*kind, text))
                        .map(|(i, (_, kind, text))| {
                            assistant_section(kind, text, true, i * 2 + 1, store, &entity, &t, &ty)
                                .into_any_element()
                        }),
                )
                .children(store.arg_stream.iter().map(|(call_id, name, text)| {
                    if tool_status_only(name) {
                        let title = tool_summary(name, &ToolCallState::Declared)
                            .map_or_else(|| format!("正在准备 {name}"), str::to_owned);
                        return tool_status_line(title, &t).into_any_element();
                    }
                    let expanded = store.expanded_tools.contains(call_id);
                    div()
                        .child(tool_toggle(&entity, call_id, expanded, format!("正在准备 {name}"), &t))
                        .children(expanded.then(|| {
                            div().px_2().text_xs().text_color(t.dim)
                                .child(SharedString::from(text.clone()))
                        }))
                        .into_any_element()
                }))
                .children(store.tools.iter().filter_map(|tool| {
                    if referenced_tools.contains(&tool.call_id) {
                        return None;
                    }
                    Some(tool_card(cx, store, &entity, tool, &t, tail_max).into_any_element())
                }))
                .children(store.subagents.iter().map(|subagent| {
                    let status = subagent
                        .status
                        .as_ref()
                        .map(|status| format!("{status:?}"))
                        .unwrap_or_else(|| "进行中".into());
                    let mut row = div()
                        .text_xs()
                        .text_color(t.dim)
                        .child(SharedString::from(format!(
                            "🤖 子代理 {} · {status}",
                            subagent.child_session_id.0,
                        )));
                    if let Some(result) = &subagent.result {
                        let head: String = content_text(result).chars().take(160).collect();
                        row = row.child(div().text_xs().text_color(t.dim).child(head));
                    }
                    row.into_any_element()
                }));
            footer.into_any_element()
        })
        .flex_1()
        .min_h_0()
        .w_full()
        .max_w(px(ty.content_width + 24.))
        .mx_auto()
        .p_3();

        // composer:组件库 Textarea(auto_grow,Enter 提交);受阻原因降为
        // 输入框下方常驻提示行(占位符固定为输入引导)。
        let blocked = self.send_block_reason();
        let composer = div()
            .mx_3()
            .mb_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(Textarea::new(&self.composer))
            .children(blocked.map(|reason| {
                div().text_xs().text_color(t.warn).child(SharedString::from(reason))
            }));

        let mut footer = div()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(t.border)
            .bg(t.panel)
            .px_3()
            .py_2();
        for interaction in self.interactions.clone().iter() {
            if interaction.resolution.is_some() {
                continue;
            }
            match interaction.kind {
                InteractionKind::Permission => {
                    let Some(call_id) = &interaction.call_id else {
                        continue;
                    };
                    let call = call_id.0.clone();
                    let allow = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(call.clone(), true, false);
                                cx.notify();
                            },
                        )
                    };
                    let trust = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(call.clone(), true, true);
                                cx.notify();
                            },
                        )
                    };
                    let deny = {
                        let call = call.clone();
                        cx.listener(
                            move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                                store.respond_permission(call.clone(), false, false);
                                cx.notify();
                            },
                        )
                    };
                    let uid = interaction.interaction_id.0.clone();
                    footer = footer.child(interaction_card(
                        "工具权限",
                        content_text(&interaction.request),
                        vec![
                            action_button(cx, format!("{uid}-allow"), "允许一次", t.ok, allow, &t)
                                .into_any_element(),
                            action_button(cx, format!("{uid}-trust"), "始终允许", t.ok, trust, &t)
                                .into_any_element(),
                            action_button(cx, format!("{uid}-deny"), "拒绝", t.bad, deny, &t)
                                .into_any_element(),
                        ],
                        Vec::new(),
                        &t,
                    ));
                }
                InteractionKind::Plan => {
                    let iid = interaction.interaction_id.0.clone();
                    let approve = cx.listener(
                        move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                            store.respond_plan(iid.clone(), true);
                            cx.notify();
                        },
                    );
                    let iid = interaction.interaction_id.0.clone();
                    let reject = cx.listener(
                        move |store: &mut SessionStore, _, _, cx: &mut Context<SessionStore>| {
                            store.respond_plan(iid.clone(), false);
                            cx.notify();
                        },
                    );
                    let uid = interaction.interaction_id.0.clone();
                    footer = footer.child(interaction_card(
                        "计划评审",
                        content_text(&interaction.request),
                        vec![
                            action_button(cx, format!("{uid}-approve"), "批准", t.ok, approve, &t)
                                .into_any_element(),
                            action_button(cx, format!("{uid}-reject"), "拒绝", t.bad, reject, &t)
                                .into_any_element(),
                        ],
                        Vec::new(),
                        &t,
                    ));
                }
                InteractionKind::Ask => {
                    // 应答内容在模态 Dialog 里;footer 只留重开入口。
                    let entity = cx.entity();
                    let iid = interaction.interaction_id.clone();
                    footer = footer.child(
                        action_button(cx,
                            format!("ask-open-{}", iid.0),
                            "回答提问",
                            t.warn,
                            move |_, window: &mut Window, cx: &mut gpui::App| {
                                open_ask_dialog(&entity, iid.clone(), window, cx);
                            },
                            &t,
                        )
                        .into_any_element(),
                    );
                }
            }
        }
        let conversation = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(header)
            .child(crate::desktop::session_controls(self, cx, &t))
            .children(crate::desktop::search(self, cx))
            .child(timeline)
            .children(crate::code_panel::review_button(self, &t, cx))
            .child(composer)
            .child(crate::desktop::composer_tools(self, cx, &t))
            .child(footer);
        div().flex().flex_1().min_w_0()
            .child(conversation)
            .children(crate::code_panel::render(self, &t, &ty, cx))
    }
}
