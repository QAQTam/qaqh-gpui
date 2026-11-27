//! 工作台视图(DESIGN LAW v3.0 · v5.0.0):
//! - 侧栏中性胶囊行(去冗余灰点);
//! - 右侧弹簧 Inspector(任务进度条 + 团队协作板合一);
//! - 居中悬浮模态设置工作台(分段滑块 + 风格/动效切换) + 新建会话面板。

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ClickEvent, Context, Div, FontWeight, SharedString, Window, div, px,
};
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Icon, Sizable, Size, WindowExt as _};
use gpui_kit::assets::IconName;
use qaqh_profile::ConfigPatch;
use qaqh_gpui_kit::kit::{
    BtnKind, RADIUS_CHIP, RADIUS_CONTROL, button, pill, segmented_item, segmented_strip,
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
use qaqh_types::SessionRunStatus;

use crate::daemon::DaemonStore;

/// 侧栏条目:中性悬浮胶囊;仅运行中/等待授权/失败时显示状态点,历史空闲会话干净留白。
#[allow(clippy::too_many_arguments)]
pub(crate) fn sidebar_row(
    row: &SessionRow,
    display_title: &str,
    parent_title: Option<SharedString>,
    anchors: qaqh_gpui_session::handoff::HandoffAnchors,
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
    let anchor_id = row.id.clone();
    let lineage_badge = row.handoff_from_session_id.as_ref().map(|_| {
        let tip = match &parent_title {
            Some(title) if !title.is_empty() => format!("交接自「{title}」"),
            _ => "由上游会话交接而来".to_owned(),
        };
        div()
            .id(SharedString::from(format!("sess-fork-{}", row.id)))
            .flex_shrink_0()
            .flex()
            .items_center()
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .child(
                Icon::new(IconName::GitFork)
                    .with_size(Size::XSmall)
                    .text_color(t.faint),
            )
    });
    div()
        .id(SharedString::from(format!("sess-{}", row.id)))
        .relative()
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
        .children(lineage_badge)
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .text_color(t.faint)
                .child(qaqh_gpui_kit::kit::relative_time(row.updated_at)),
        )
        .child(qaqh_gpui_session::handoff::anchor_probe(move |bounds| {
            anchors.record_sidebar_row(&anchor_id, bounds);
        }))
}

/// 右侧弹簧团队任务面板(仅当当前会话存在团队协作任务时渲染)。
pub(crate) fn inspector_dock(
    daemon: &mut DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
) -> Option<AnyElement> {
    let current_entity = daemon.current.clone()?;
    let t = *cx.colors();
    let current_seed = current_entity.read(cx).session_id.clone();

    let team_tasks = match &daemon.team {
        Some((seed, resp)) if seed == &current_seed && !resp.tasks.tasks.is_empty() => {
            &resp.tasks.tasks
        }
        _ => return None,
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
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Users)
                        .with_size(Size::XSmall)
                        .text_color(t.accent),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child("团队任务"),
                ),
        )
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

    let mut list = div()
        .id("team-list")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .p_2p5()
        .flex()
        .flex_col()
        .gap_1p5();
    for task in team_tasks {
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
    let body = list.into_any_element();

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

    let page_title = match daemon.settings_editor {
        SettingsEditor::Local => "本机偏好",
        SettingsEditor::Config => "全局策略",
        SettingsEditor::Catalog => "服务商与模型",
        SettingsEditor::Accounts => "账号与额度",
        SettingsEditor::Session => "当前会话",
        SettingsEditor::Workspaces => "工作区",
        SettingsEditor::Archived => "已归档会话",
        SettingsEditor::About => "关于",
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

        let work_trace_switch = Switch::new("local-collapse-work-trace")
            .checked(behavior.collapse_work_trace)
            .small()
            .on_change(|checked, _, cx| {
                let val = *checked;
                settings::update(cx, |content| content.behavior.collapse_work_trace = val);
            })
            .into_any_element();

        let live_patch_switch = Switch::new("exp-live-patch-preview")
            .checked(behavior.exp_live_patch_preview)
            .small()
            .on_change(|checked, _, cx| {
                let val = *checked;
                settings::update(cx, |content| {
                    content.behavior.exp_live_patch_preview = val;
                });
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
                        "",
                        segmented_strip(preset_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "主题模式",
                        "",
                        segmented_strip(theme_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "界面动效",
                        "",
                        segmented_strip(motion_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "时间线留白",
                        "",
                        segmented_strip(density_buttons, &t).into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "终端配色方案",
                        "",
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
                        "",
                        font_size_stepper.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "界面字体",
                        "留空使用系统默认字体",
                        font_input.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "代码字号",
                        "",
                        code_size_stepper.into_any_element(),
                        &t,
                    ),
                    setting_row(
                        "正文行高",
                        "",
                        line_height_stepper.into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ))
            .child(section_card(
                "交互与输出",
                vec![
                    setting_row("自动跟随底部", "", follow_switch, &t),
                    setting_row(
                        "折叠过程轨迹视窗",
                        "把思考链、工具调用与已被新正文替代的中间回复收进限高滚动视窗",
                        work_trace_switch,
                        &t,
                    ),
                    setting_row(
                        "工具输出尾窗上限",
                        "",
                        segmented_strip(tail_buttons, &t).into_any_element(),
                        &t,
                    ),
                ],
                &t,
            ))
            .child(section_card(
                "实验性功能 (Exp)",
                vec![setting_row(
                    "实时补丁跟随预览",
                    "apply_patch 开始流式输出时自动右侧展开目标文件并锁定行号预演改动（失败自动置灰）",
                    live_patch_switch,
                    &t,
                )],
                &t,
            ))
            .child(section_card(
                "Daemon 运维",
                vec![
                    setting_row("热重载会话配置", "", reload_control, &t),
                    setting_row("重连本地 Daemon", "", reconnect_control, &t),
                    setting_row("导出诊断信息", "", diag_control, &t),
                ],
                &t,
            ))
    } else {
        crate::settings_ui::render(daemon, cx, &t)
    };

    let header_bar = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .pb_4()
        .mb_5()
        .border_b_1()
        .border_color(t.border_subtle)
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(t.text)
                .child(page_title),
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

    let op_banner = daemon.last_operation.as_ref().map(|message| {
        div()
            .mb_4()
            .px_3()
            .py_2()
            .rounded(px(RADIUS_CONTROL))
            .bg(qaqh_gpui_kit::kit::soft(t.bad, 0.10))
            .border_1()
            .border_color(qaqh_gpui_kit::kit::soft(t.bad, 0.28))
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_xs()
                    .text_color(t.bad)
                    .child(SharedString::from(message.clone())),
            )
            .child(
                div()
                    .id("dismiss-settings-error")
                    .size(px(20.))
                    .rounded(px(RADIUS_CHIP))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|daemon: &mut DaemonStore, _, _, cx| {
                        daemon.last_operation = None;
                        cx.notify();
                    }))
                    .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.bad)),
            )
    });

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
                        .children(op_banner)
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
    setting_row("权限档位", "", control.into_any_element(), t)
}
