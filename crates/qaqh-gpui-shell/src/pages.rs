//! Overcoder v5.0 主导航页面(DESIGN LAW v3.0 · v5.0.0):
//! - `startup_page`: 内联 StartupView(Antigravity 极简四锚点 + 错峰滑入建议行 + 双层统一 Composer);
//! - `tools_page`: 「工具」页(daemon `skills.catalog` 分组诊断 + `accordion_reveal` + `MCP` 只读表);
//! - `team_page`: 「团队」页(会话切换 Popover + KPI 统计 + 任务板依赖分析 + 树形花名册与状态光点);
//! - `access_page`: 「接入」页(全局模型目录概览 + 复用 `settings_ui::accounts` SIWC / DeepSeek 账号管理)。

use gpui_kit::prelude::*;
use gpui_kit::{
    Anchor, AnyElement, Context, Div, FontWeight, Hsla, InteractiveElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, WeakEntity, Window, div, px,
};
use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::{Disableable, Icon, Sizable, Size};
use qaqh_gpui_kit::kit::{
    BtnKind, RADIUS_CARD, RADIUS_CHIP, RADIUS_COMPOSER, RADIUS_CONTROL, RADIUS_POPOVER, button,
    card, pill, segmented_item, segmented_strip, shadow_composer, shadow_popover, soft,
};
use qaqh_gpui_kit::motion::{
    accordion_reveal, effective_motion, live_dot, shimmer_label, spring_chevron, stagger_enter,
};
use qaqh_gpui_kit::settings::MotionLevel;
use qaqh_gpui_kit::theme::Palette;
use qaqh_gpui_session::store::{SettingsEditor, model_default_effort, model_effort_levels};
use qaqh_gpui_wire::bridge::{BridgeRequest, ConnectionState};
use qaqh_core_vocab::{ModelSelection, ToolMode};
use qaqh_ringing_v3::{SkillCatalogEntry, TaskState, TeamAgentStatus};

use crate::daemon::{DaemonStore, ToolsTab, clean_session_title};

fn token_badge(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{}M", tokens / 1_000_000)
    } else {
        format!("{}k", tokens / 1_000)
    }
}

fn tools_mode_label(mode: Option<ToolMode>) -> &'static str {
    match mode {
        Some(ToolMode::Core) => "精简",
        Some(ToolMode::Custom) => "自定义",
        Some(ToolMode::Standard) | None => "标准",
    }
}

fn popover_trigger(
    cx: &gpui_kit::App,
    id: &'static str,
    icon: IconName,
    label: impl Into<SharedString>,
    tooltip: &'static str,
    t: &Palette,
) -> Button {
    Button::new(id)
        .with_size(Size::XSmall)
        .rounded(ButtonRounded::Size(px(RADIUS_CHIP)))
        .compact()
        .dropdown_caret(true)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(t.surface_subtle)
                .foreground(t.dim)
                .hover(t.hover)
                .active(t.selected),
        )
        .icon(Icon::new(icon).with_size(Size::XSmall).text_color(t.dim))
        .label(label)
        .tooltip(tooltip)
}

fn popover_card(t: &Palette, width: f32) -> Div {
    div()
        .w(px(width))
        .rounded(px(RADIUS_POPOVER))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .shadow(shadow_popover(t))
        .p_1p5()
        .flex()
        .flex_col()
        .gap_0p5()
}

fn popover_option_row(
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    selected: bool,
    t: &Palette,
    on_pick: impl Fn(&mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .px_2()
        .py_1p5()
        .rounded(px(RADIUS_CHIP))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .cursor_pointer()
        .when(selected, |row| row.bg(t.accent_soft).text_color(t.accent))
        .when(!selected, |row| {
            row.text_color(t.text)
                .hover(|style| style.bg(t.hover))
        })
        .on_click(move |_, window, cx| on_pick(window, cx))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .truncate()
                        .child(title),
                )
                .children(subtitle.map(|sub| {
                    div()
                        .text_xs()
                        .text_color(if selected {
                            soft(t.accent, 0.85)
                        } else {
                            t.faint
                        })
                        .truncate()
                        .child(sub)
                })),
        )
        .children(selected.then(|| {
            Icon::new(IconName::Check)
                .with_size(Size::XSmall)
                .text_color(t.accent)
        }))
        .into_any_element()
}

fn page_shell(
    scroll_id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    header_right: Vec<AnyElement>,
    body: Vec<AnyElement>,
    t: &Palette,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .child(
            div()
                .px_6()
                .py_4()
                .border_b_1()
                .border_color(t.border_subtle)
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .min_w_0()
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .truncate()
                                .child(subtitle),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .flex_shrink_0()
                        .children(header_right),
                ),
        )
        .child(
            div()
                .id(scroll_id)
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px_6()
                .pt_5()
                .pb_10()
                .child(
                    div()
                        .w_full()
                        .max_w(px(860.))
                        .mx_auto()
                        .flex()
                        .flex_col()
                        .gap_5()
                        .children(body),
                ),
        )
}

// ============================================================================
// 1. 内联 StartupView(主页空态 / 新对话)
// ============================================================================

const SUGGESTIONS: [(IconName, &str, &str); 3] = [
    (
        IconName::GitBranch,
        "分析当前项目的模块架构与核心调用流",
        "请系统性分析当前项目的代码架构、核心模块职责与依赖流向。",
    ),
    (
        IconName::Bug,
        "审查近期变更并排查潜在缺陷",
        "请检查最近的代码变更，排查潜在的边界条件漏洞或状态冲突。",
    ),
    (
        IconName::Gauge,
        "定位性能热点与内存开销并给出优化方案",
        "请分析项目的性能热点与资源占用，并给出针对性的优化方案。",
    ),
];

pub(crate) fn startup_page(
    daemon: &DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> AnyElement {
    let conn_banner = match &daemon.connection {
        ConnectionState::Ready { .. } => None,
        ConnectionState::Connecting => Some(
            div()
                .px_3()
                .py_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(soft(t.warn, 0.10))
                .border_1()
                .border_color(soft(t.warn, 0.25))
                .flex()
                .items_center()
                .gap_2()
                .child(live_dot("startup-conn", t.warn, true))
                .child(
                    div()
                        .text_xs()
                        .text_color(t.warn)
                        .child("正在连接本地 daemon…"),
                )
                .into_any_element(),
        ),
        ConnectionState::Failed { detail } => Some(
            div()
                .px_3()
                .py_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(soft(t.bad, 0.08))
                .border_1()
                .border_color(soft(t.bad, 0.24))
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(live_dot("startup-conn-err", t.bad, false))
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.bad)
                                .truncate()
                                .child(SharedString::from(detail.clone())),
                        ),
                )
                .child(button(
                    cx,
                    BtnKind::Soft,
                    "startup-reconnect",
                    "重试连接",
                    t,
                    cx.listener(|daemon, _, _, cx| {
                        daemon.shell_error = None;
                        let _ = daemon.requests.unbounded_send(BridgeRequest::Reconnect);
                        cx.notify();
                    }),
                ))
                .into_any_element(),
        ),
    };

    let hero = div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2p5()
                .child(
                    div()
                        .size(px(34.))
                        .rounded(px(10.))
                        .bg(t.surface_subtle)
                        .border_1()
                        .border_color(t.border_subtle)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            Icon::new(IconName::Sparkles)
                                .with_size(Size::Small)
                                .text_color(t.text),
                        ),
                )
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text)
                        .child("QAQHarness"),
                ),
        )
        .child(
            div()
                .text_sm()
                .text_color(t.dim)
                .child("选定工作区，直接描述你的目标，立刻开启原生工程协作。"),
        );

    let mut suggestion_rows = Vec::with_capacity(SUGGESTIONS.len());
    for (idx, (icon, title, prompt)) in SUGGESTIONS.into_iter().enumerate() {
        let row = div()
            .id(SharedString::from(format!("startup-sug-{idx}")))
            .w_full()
            .px_2p5()
            .py_2()
            .rounded(px(RADIUS_CONTROL))
            .flex()
            .items_center()
            .gap_2p5()
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(move |daemon, _, window, cx| {
                daemon.startup_input.update(cx, |field, cx| {
                    field.set_value(prompt, window, cx);
                    field.focus(window, cx);
                });
                cx.notify();
            }))
            .child(
                Icon::new(icon)
                    .with_size(Size::XSmall)
                    .text_color(t.faint),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(t.dim)
                    .truncate()
                    .child(title),
            )
            .into_any_element();
        suggestion_rows.push(stagger_enter(
            format!("startup-sug-stagger-{}-{idx}", daemon.page_epoch),
            idx,
            SUGGESTIONS.len(),
            row,
            window,
            cx,
        ));
    }

    let composer_island = startup_composer_island(daemon, cx, t);

    div()
        .id("startup-scroll")
        .flex_1()
        .min_w_0()
        .min_h_0()
        .overflow_y_scroll()
        .px_6()
        .py_8()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .child(
            div()
                .w_full()
                .max_w(px(720.))
                .flex()
                .flex_col()
                .gap_5()
                .children(conn_banner)
                .child(hero)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .children(suggestion_rows),
                )
                .child(composer_island),
        )
        .into_any_element()
}

fn startup_composer_island(
    daemon: &DaemonStore,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Div {
    let weak = cx.entity().downgrade();
    let t_copy = *t;

    // 1) 工作区选择器 Popover
    let active_ws_idx = daemon.new_workspace;
    let ws_label = daemon
        .workspaces
        .get(active_ws_idx)
        .map(|w| w.title.clone())
        .unwrap_or_else(|| "默认工作区".to_owned());
    let ws_short: String = ws_label.chars().take(28).collect();
    let ws_entries: Vec<(usize, String, String)> = daemon
        .workspaces
        .iter()
        .enumerate()
        .map(|(idx, w)| (idx, w.title.clone(), w.path.clone()))
        .collect();
    let ws_weak = weak.clone();
    let ws_popover = Popover::new("startup-ws-popover")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .trigger(popover_trigger(
            cx,
            "startup-ws-btn",
            IconName::Folder,
            ws_short,
            "选择新会话归属的工作区目录",
            t,
        ))
        .content(move |_state, _window, cx| {
            let popover: WeakEntity<PopoverState> = cx.entity().downgrade();
            let mut card_box = popover_card(&t_copy, 290.);
            card_box = card_box.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t_copy.faint)
                    .child("工作区目录"),
            );
            let mut scroll = div()
                .id("startup-ws-list")
                .max_h(px(260.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_0p5();
            for (idx, title, path) in &ws_entries {
                let idx = *idx;
                let selected = active_ws_idx == idx;
                let w = ws_weak.clone();
                let pop = popover.clone();
                scroll = scroll.child(popover_option_row(
                    SharedString::from(format!("startup-ws-opt-{idx}")),
                    SharedString::from(title.clone()),
                    Some(SharedString::from(path.clone())),
                    selected,
                    &t_copy,
                    move |window, cx| {
                        let _ = w.update(cx, |daemon, cx| {
                            daemon.new_workspace = idx;
                            cx.notify();
                        });
                        let _ = pop.update(cx, |state, cx| state.dismiss(window, cx));
                    },
                ));
            }
            let default_selected = active_ws_idx >= ws_entries.len();
            let w_default = ws_weak.clone();
            let pop_default = popover.clone();
            scroll = scroll.child(popover_option_row(
                "startup-ws-opt-default".into(),
                "默认工作区".into(),
                Some("使用 daemon 默认工作目录".into()),
                default_selected,
                &t_copy,
                move |window, cx| {
                    let _ = w_default.update(cx, |daemon, cx| {
                        daemon.new_workspace = usize::MAX;
                        cx.notify();
                    });
                    let _ = pop_default.update(cx, |state, cx| state.dismiss(window, cx));
                },
            ));
            let w_manage = ws_weak.clone();
            let pop_manage = popover.clone();
            card_box
                .child(scroll)
                .child(div().my_1().h(px(1.)).bg(t_copy.border_subtle))
                .child(popover_option_row(
                    "startup-ws-manage".into(),
                    "管理工作区…".into(),
                    None,
                    false,
                    &t_copy,
                    move |window, cx| {
                        let _ = pop_manage.update(cx, |state, cx| state.dismiss(window, cx));
                        let _ = w_manage.update(cx, |daemon, cx| {
                            daemon.open_settings(SettingsEditor::Workspaces, window, cx);
                        });
                    },
                ))
        })
        .into_any_element();

    // 2) 模型选择器 Popover
    let current_model = daemon.new_model.clone();
    let model_label = match (&daemon.providers, &current_model) {
        (Some(catalog), Some(sel)) => catalog
            .providers
            .get(&sel.provider_id)
            .and_then(|p| p.models.get(&sel.model_id))
            .and_then(|m| m.display_name.as_deref())
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| sel.model_id.clone()),
        (_, Some(sel)) => sel.model_id.clone(),
        (_, None) => "默认模型".to_owned(),
    };
    let model_short: String = model_label.chars().take(24).collect();
    #[derive(Clone)]
    struct ModelGroup {
        provider_name: String,
        unauthenticated: bool,
        items: Vec<(ModelSelection, String, String)>,
    }
    let model_groups: Vec<ModelGroup> = daemon
        .providers
        .as_ref()
        .map(|catalog| {
            catalog
                .providers
                .iter()
                .filter_map(|(pid, provider)| {
                    let mut items = Vec::new();
                    for (mid, model) in &provider.models {
                        let selection = ModelSelection {
                            provider_id: pid.clone(),
                            model_id: mid.clone(),
                            options: Default::default(),
                        };
                        if qaqh_profile::resolve_model_selection(
                            catalog,
                            &qaqh_gpui_session::store::profile_selection(&selection),
                        )
                        .is_ok()
                        {
                            let display = model
                                .display_name
                                .as_deref()
                                .filter(|s| !s.is_empty())
                                .unwrap_or(mid)
                                .to_owned();
                            let ctx = format!("{} ctx", token_badge(model.context_window));
                            items.push((selection, display, ctx));
                        }
                    }
                    let unauthenticated = match qaqh_gpui_wire::profile_vocab::credential_kind(provider) {
                        qaqh_core_vocab::CredentialSource::ApiKey => false,
                        qaqh_core_vocab::CredentialSource::ChatgptPlan => !daemon.siwc_ready(),
                        qaqh_core_vocab::CredentialSource::DeepseekAccount => {
                            !daemon.dsaccount_signed_in()
                        }
                    };
                    (!items.is_empty()).then(|| ModelGroup {
                        provider_name: if provider.name.is_empty() {
                            pid.clone()
                        } else {
                            provider.name.clone()
                        },
                        unauthenticated,
                        items,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let m_weak = weak.clone();
    let m_current = current_model.clone();
    let model_popover = Popover::new("startup-model-popover")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .trigger(popover_trigger(
            cx,
            "startup-model-btn",
            IconName::Cpu,
            model_short,
            "选择新会话初始模型",
            t,
        ))
        .content(move |_state, _window, cx| {
            let popover: WeakEntity<PopoverState> = cx.entity().downgrade();
            let mut card_box = popover_card(&t_copy, 280.);
            let mut list = div()
                .id("startup-model-list")
                .max_h(px(280.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1();
            let w_inherit = m_weak.clone();
            let pop_inherit = popover.clone();
            list = list.child(popover_option_row(
                "startup-model-inherit".into(),
                "跟随全局默认".into(),
                Some("继承 daemon 默认配置".into()),
                m_current.is_none(),
                &t_copy,
                move |window, cx| {
                    let _ = w_inherit.update(cx, |daemon, cx| {
                        daemon.new_model = None;
                        cx.notify();
                    });
                    let _ = pop_inherit.update(cx, |state, cx| state.dismiss(window, cx));
                },
            ));
            for group in &model_groups {
                list = list
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t_copy.faint)
                                    .child(group.provider_name.clone()),
                            )
                            .children(group.unauthenticated.then(|| {
                                div()
                                    .text_size(px(10.))
                                    .text_color(t_copy.warn)
                                    .child("未登录")
                            })),
                    )
                    .children(group.items.iter().map(|(selection, display, ctx)| {
                        let unauthenticated = group.unauthenticated;
                        let is_selected = m_current.as_ref().is_some_and(|cur| {
                            cur.provider_id == selection.provider_id
                                && cur.model_id == selection.model_id
                        });
                        let subtitle = if unauthenticated {
                            "需先登录对应账号".to_owned()
                        } else {
                            ctx.clone()
                        };
                        let sel = selection.clone();
                        let w = m_weak.clone();
                        let pop = popover.clone();
                        popover_option_row(
                            SharedString::from(format!(
                                "startup-model-{}-{}",
                                sel.provider_id, sel.model_id
                            )),
                            SharedString::from(display.clone()),
                            Some(SharedString::from(subtitle)),
                            is_selected,
                            &t_copy,
                            move |window, cx| {
                                let sel = sel.clone();
                                let _ = w.update(cx, |daemon, cx| {
                                    if unauthenticated {
                                        daemon.navigate(
                                            crate::daemon::PrimarySection::Access,
                                            window,
                                            cx,
                                        );
                                    } else {
                                        daemon.new_model = Some(sel);
                                        cx.notify();
                                    }
                                });
                                let _ = pop.update(cx, |state, cx| state.dismiss(window, cx));
                            },
                        )
                    }));
            }
            let w_cfg = m_weak.clone();
            let pop_cfg = popover.clone();
            card_box = card_box
                .child(list)
                .child(div().my_1().h(px(1.)).bg(t_copy.border_subtle))
                .child(popover_option_row(
                    "startup-model-manage".into(),
                    "配置服务商与模型…".into(),
                    None,
                    false,
                    &t_copy,
                    move |window, cx| {
                        let _ = pop_cfg.update(cx, |state, cx| state.dismiss(window, cx));
                        let _ = w_cfg.update(cx, |daemon, cx| {
                            daemon.open_settings(SettingsEditor::Catalog, window, cx);
                        });
                    },
                ));
            card_box
        })
        .into_any_element();

    // 3) 推理强度 Popover(仅当前所选模型或继承的全局默认模型支持推理档位时显示)
    let effective_model: Option<ModelSelection> = daemon
        .new_model
        .clone()
        .or_else(|| {
            daemon
                .providers
                .as_deref()
                .and_then(|catalog| catalog.default_model_selection.as_ref())
                .map(qaqh_gpui_session::store::selection_from_profile)
        });
    let levels = daemon
        .providers
        .as_deref()
        .zip(effective_model.as_ref())
        .map(|(catalog, model)| model_effort_levels(catalog, model))
        .unwrap_or_default();
    let default_effort = daemon
        .providers
        .as_deref()
        .zip(effective_model.as_ref())
        .and_then(|(catalog, model)| model_default_effort(catalog, model));
    let effort_popover = (!levels.is_empty()).then(|| {
        let current_effort = effective_model
            .as_ref()
            .and_then(|m| m.options.reasoning_effort.clone());
        let effort_label = match (&current_effort, &default_effort) {
            (Some(effort), _) => effort.clone(),
            (None, Some(def)) => format!("默认 · {def}"),
            (None, None) => "默认".to_owned(),
        };
        let e_weak = weak.clone();
        let base_model = effective_model.clone();
        Popover::new("startup-effort-popover")
            .anchor(Anchor::BottomLeft)
            .appearance(false)
            .trigger(popover_trigger(
                cx,
                "startup-effort-btn",
                IconName::SlidersHorizontal,
                effort_label,
                "选择推理思考深度",
                t,
            ))
            .content(move |_state, _window, cx| {
                let popover: WeakEntity<PopoverState> = cx.entity().downgrade();
                let mut card_box = popover_card(&t_copy, 180.);
                card_box = card_box.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t_copy.faint)
                        .child("推理强度"),
                );
                let default_sub = default_effort
                    .as_deref()
                    .map(|def| format!("模型默认 ({def})"));
                let mut opts: Vec<(Option<String>, String, Option<String>)> =
                    vec![(None, "默认".to_owned(), default_sub)];
                for lvl in &levels {
                    opts.push((Some(lvl.clone()), lvl.clone(), None));
                }
                for (val, text, subtitle) in opts {
                    let selected = current_effort == val;
                    let w = e_weak.clone();
                    let pop = popover.clone();
                    let fallback = base_model.clone();
                    card_box = card_box.child(popover_option_row(
                        SharedString::from(format!("startup-effort-{text}")),
                        SharedString::from(text),
                        subtitle.map(SharedString::from),
                        selected,
                        &t_copy,
                        move |window, cx| {
                            let val = val.clone();
                            let fallback = fallback.clone();
                            let _ = w.update(cx, |daemon, cx| {
                                if let Some(model) = &mut daemon.new_model {
                                    model.options.reasoning_effort = val;
                                    cx.notify();
                                } else if let Some(mut model) = fallback {
                                    model.options.reasoning_effort = val;
                                    daemon.new_model = Some(model);
                                    cx.notify();
                                }
                            });
                            let _ = pop.update(cx, |state, cx| state.dismiss(window, cx));
                        },
                    ));
                }
                card_box
            })
            .into_any_element()
    });

    // 4) 工具档位 Popover
    let cur_mode = daemon.startup_tool_mode;
    let mode_weak = weak;
    let tools_popover = Popover::new("startup-tools-popover")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .trigger(popover_trigger(
            cx,
            "startup-tools-btn",
            IconName::Wrench,
            tools_mode_label(cur_mode),
            "选择新会话工具预设",
            t,
        ))
        .content(move |_state, _window, cx| {
            let popover: WeakEntity<PopoverState> = cx.entity().downgrade();
            let presets: [(Option<ToolMode>, &'static str, &'static str); 3] = [
                (None, "标准工具", "完整工程读写、搜索与终端"),
                (Some(ToolMode::Core), "精简", "基础文件读写与命令执行"),
                (Some(ToolMode::Custom), "自定义", "只保留工具白名单内的工具"),
            ];
            let mut card_box = popover_card(&t_copy, 224.);
            card_box = card_box.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t_copy.faint)
                    .child("工具权限预设"),
            );
            for (val, label, desc) in presets {
                let selected = cur_mode == val;
                let w = mode_weak.clone();
                let pop = popover.clone();
                card_box = card_box.child(popover_option_row(
                    SharedString::from(format!("startup-mode-{label}")),
                    label.into(),
                    Some(desc.into()),
                    selected,
                    &t_copy,
                    move |window, cx| {
                        let _ = w.update(cx, |daemon, cx| {
                            daemon.startup_tool_mode = val;
                            cx.notify();
                        });
                        let _ = pop.update(cx, |state, cx| state.dismiss(window, cx));
                    },
                ));
            }
            card_box
        })
        .into_any_element();

    let empty_input = daemon.startup_input.read(cx).value().trim().is_empty();
    let send_disabled = empty_input || daemon.creating_session;
    let send_btn = Button::new("startup-send")
        .with_size(Size::Medium)
        .rounded(ButtonRounded::Size(px(999.)))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(t.accent)
                .foreground(t.on_accent)
                .hover(t.accent_hover)
                .active(t.accent_hover),
        )
        .icon(
            Icon::new(if daemon.creating_session {
                IconName::LoaderCircle
            } else {
                IconName::ArrowUp
            })
            .text_color(t.on_accent),
        )
        .tooltip("发送首条消息并创建对话")
        .when(send_disabled, |b| b.disabled(true))
        .on_click(cx.listener(|daemon, _, _, cx| {
            daemon.submit_startup(cx);
        }));

    div()
        .w_full()
        .rounded(px(RADIUS_COMPOSER))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .shadow(shadow_composer(t, daemon.creating_session))
        .overflow_hidden()
        .flex()
        .flex_col()
        // 第一层:顶部上下文条(工作区选择 + 建空会话)
        .child(
            div()
                .px_3()
                .py_1p5()
                .bg(t.surface_subtle)
                .border_b_1()
                .border_color(t.border_subtle)
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(ws_popover)
                .child(
                    div()
                        .id("startup-blank-session")
                        .px_2()
                        .py_1()
                        .rounded(px(RADIUS_CHIP))
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_xs()
                        .text_color(t.dim)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .on_click(cx.listener(|daemon, _, _, cx| {
                            daemon.create_session();
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::Plus).with_size(Size::XSmall))
                        .child(if daemon.creating_session {
                            "创建中…"
                        } else {
                            "仅建空会话"
                        }),
                ),
        )
        // 第二层:无框文本域 + 底部控制栏
        .child(
            div()
                .px_3()
                .pt_2p5()
                .pb_1()
                .child(Textarea::new(&daemon.startup_input).appearance(false)),
        )
        .child(
            div()
                .px_2p5()
                .pb_2p5()
                .flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(model_popover)
                        .children(effort_popover)
                        .child(tools_popover),
                )
                .child(send_btn),
        )
}

// ============================================================================
// 2. 「工具」页 (Skills catalog + MCP 只读视图)
// ============================================================================

pub(crate) fn tools_page(
    daemon: &DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> AnyElement {
    let tab_strip = segmented_strip(
        vec![
            segmented_item(
                "tools-tab-skills",
                "Skills",
                daemon.tools_tab == ToolsTab::Skills,
                t,
                cx.listener(|daemon, _, _, cx| {
                    if daemon.tools_tab != ToolsTab::Skills {
                        daemon.tools_tab = ToolsTab::Skills;
                        daemon.page_epoch = daemon.page_epoch.wrapping_add(1);
                        cx.notify();
                    }
                }),
            ),
            segmented_item(
                "tools-tab-mcp",
                "MCP",
                daemon.tools_tab == ToolsTab::Mcp,
                t,
                cx.listener(|daemon, _, _, cx| {
                    if daemon.tools_tab != ToolsTab::Mcp {
                        daemon.tools_tab = ToolsTab::Mcp;
                        daemon.page_epoch = daemon.page_epoch.wrapping_add(1);
                        let _ = daemon
                            .requests
                            .unbounded_send(BridgeRequest::Config(qaqh_ringing_v3::ConfigRequest::LoadFull));
                        cx.notify();
                    }
                }),
            ),
            segmented_item(
                "tools-tab-catalog",
                "工具目录",
                daemon.tools_tab == ToolsTab::Catalog,
                t,
                cx.listener(|daemon, _, _, cx| {
                    if daemon.tools_tab != ToolsTab::Catalog {
                        daemon.tools_tab = ToolsTab::Catalog;
                        daemon.page_epoch = daemon.page_epoch.wrapping_add(1);
                        daemon.refresh_tool_catalog(cx);
                        cx.notify();
                    }
                }),
            ),
        ],
        t,
    )
    .into_any_element();

    let rescan_btn = button(
        cx,
        BtnKind::Soft,
        "tools-rescan",
        if daemon.skills_scanning {
            "扫描中…"
        } else {
            "重新扫描"
        },
        t,
        cx.listener(|daemon, _, _, cx| {
            daemon.refresh_skills();
            daemon.refresh_tool_catalog(cx);
            let _ = daemon
                .requests
                .unbounded_send(BridgeRequest::Config(qaqh_ringing_v3::ConfigRequest::LoadFull));
            cx.notify();
        }),
    );

    let header_right = vec![tab_strip, rescan_btn.into_any_element()];

    let body = match daemon.tools_tab {
        ToolsTab::Skills => tools_skills_body(daemon, window, cx, t),
        ToolsTab::Mcp => tools_mcp_body(daemon, window, cx, t),
        ToolsTab::Catalog => tools_catalog_body(daemon, cx, t),
    };

    page_shell(
        "tools-page-scroll",
        "工具",
        "会话可用的 Skills 与 MCP 工具能力，由本地 daemon 解析与校验。",
        header_right,
        body,
        t,
    )
    .into_any_element()
}

fn tools_catalog_body(
    daemon: &DaemonStore,
    cx: &Context<DaemonStore>,
    t: &Palette,
) -> Vec<AnyElement> {
    let Some(catalog) = daemon
        .current
        .as_ref()
        .and_then(|store| store.read(cx).tool_catalog.clone())
    else {
        return vec![
            card(t)
                .p_4()
                .text_xs()
                .text_color(t.dim)
                .child("选择会话后将从后端加载工具目录（ToolCatalog）。")
                .into_any_element(),
        ];
    };

    let available_count = catalog.available().count();
    let total_count = catalog.entries.len();
    let mode_label = tools_mode_label(Some(catalog.mode));

    let mut out = Vec::new();
    out.push(
        card(t)
            .px_3p5()
            .py_2p5()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text)
                    .child(SharedString::from(format!("当前模式：{mode_label}"))),
            )
            .child(pill(
                &format!("可用 {available_count} / {total_count}"),
                t.accent,
                t,
            ))
            .into_any_element(),
    );

    let cards = catalog.entries.iter().map(|entry| {
        let group = match entry.group {
            qaqh_core_vocab::ToolGroup::Read => "只读",
            qaqh_core_vocab::ToolGroup::Write => "写入",
            qaqh_core_vocab::ToolGroup::Exec => "执行",
            qaqh_core_vocab::ToolGroup::Net => "网络",
        };
        let (risk_label, risk_color) = match entry.risk {
            qaqh_core_vocab::ToolRiskLevel::ReadOnly => ("低风险", t.ok),
            qaqh_core_vocab::ToolRiskLevel::Write => ("写操作", t.warn),
            qaqh_core_vocab::ToolRiskLevel::Destructive => ("高风险", t.bad),
            qaqh_core_vocab::ToolRiskLevel::Administrative => ("管理", t.accent),
        };
        let (avail_label, avail_color) = match &entry.availability {
            qaqh_core_vocab::ToolAvailability::Available => ("可用", t.ok),
            qaqh_core_vocab::ToolAvailability::Unavailable { reason } => (
                match reason {
                    qaqh_core_vocab::ToolUnavailableReason::ModelLacksToolCalls => "模型无工具能力",
                    qaqh_core_vocab::ToolUnavailableReason::ModelLacksImageInput => "模型不支持图像",
                    qaqh_core_vocab::ToolUnavailableReason::ExcludedByToolMode => "模式已排除",
                    qaqh_core_vocab::ToolUnavailableReason::ExcludedByGlobalCeiling => "全局策略禁用",
                },
                t.faint,
            ),
        };
        let origin = match &entry.origin {
            qaqh_core_vocab::ToolOrigin::Builtin => "内置".to_owned(),
            qaqh_core_vocab::ToolOrigin::DynamicMcp { server } => format!("MCP:{server}"),
            qaqh_core_vocab::ToolOrigin::DynamicExtension { id } => format!("扩展:{id}"),
        };

        card(t)
            .px_3p5()
            .py_2p5()
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
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .truncate()
                                    .child(SharedString::from(entry.name.clone())),
                            )
                            .child(pill(group, t.dim, t))
                            .child(pill(risk_label, risk_color, t)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .flex_shrink_0()
                            .child(div().text_xs().text_color(t.faint).child(origin))
                            .child(pill(avail_label, avail_color, t)),
                    ),
            )
            .when(!entry.summary.is_empty(), |c| {
                c.child(
                    div()
                        .text_xs()
                        .text_color(t.dim)
                        .child(SharedString::from(entry.summary.clone())),
                )
            })
            .into_any_element()
    });

    out.push(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(cards)
            .into_any_element(),
    );
    out
}

fn tools_skills_body(
    daemon: &DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Vec<AnyElement> {
    let mut blocks = Vec::new();
    let animate_motion = effective_motion(cx) != MotionLevel::Off;

    if daemon.skills_scanning && daemon.skills_report.is_none() {
        blocks.push(
            card(t)
                .p_4()
                .flex()
                .items_center()
                .gap_2()
                .child(Icon::new(IconName::RefreshCw).with_size(Size::XSmall).text_color(t.dim))
                .child(shimmer_label(
                    "tools-scan-shimmer",
                    "正在扫描 daemon Skills 目录…",
                    t,
                ))
                .into_any_element(),
        );
        return blocks;
    }

    let Some(report) = &daemon.skills_report else {
        blocks.push(
            card(t)
                .p_4()
                .text_xs()
                .text_color(t.dim)
                .child("点击右上角「重新扫描」拉取 daemon Skills 目录。")
                .into_any_element(),
        );
        return blocks;
    };

    if !report.diagnostics.is_empty() {
        let issue_rows = report.diagnostics.iter().map(|issue| {
            let color = if issue.severity == "error" { t.bad } else { t.warn };
            div()
                .px_3()
                .py_2()
                .rounded(px(RADIUS_CONTROL))
                .bg(soft(color, 0.08))
                .border_1()
                .border_color(soft(color, 0.22))
                .flex()
                .items_start()
                .gap_2()
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .with_size(Size::XSmall)
                        .text_color(color),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(t.text)
                        .child(SharedString::from(format!(
                            "{} · {}",
                            issue.path, issue.message
                        ))),
                )
        });
        blocks.push(
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .children(issue_rows)
                .into_any_element(),
        );
    }

    let project_skills: Vec<SkillCatalogEntry> = report
        .skills
        .iter()
        .filter(|skill| skill.scope == "project")
        .cloned()
        .collect();
    let user_skills: Vec<SkillCatalogEntry> = report
        .skills
        .iter()
        .filter(|skill| skill.scope != "project")
        .cloned()
        .collect();
    blocks.push(skill_group_section(
        daemon,
        "project",
        "项目级",
        "工作区目录内由 daemon catalog 自动发现",
        IconName::FolderTree,
        &project_skills,
        "当前工作区 catalog 没有 project skills。",
        animate_motion,
        window,
        cx,
        t,
    ));
    blocks.push(skill_group_section(
        daemon,
        "user",
        "用户级",
        "全局用户目录内由 daemon catalog 发现",
        IconName::UserRound,
        &user_skills,
        "catalog 没有 user skills。",
        animate_motion,
        window,
        cx,
        t,
    ));
    blocks
}

#[allow(clippy::too_many_arguments)]
fn skill_group_section(
    daemon: &DaemonStore,
    group_key: &'static str,
    title: &'static str,
    hint: &'static str,
    icon: IconName,
    skills: &[SkillCatalogEntry],
    empty_text: &'static str,
    animate_motion: bool,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> AnyElement {
    let mut header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(Icon::new(icon).with_size(Size::XSmall).text_color(t.dim))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(title),
                )
                .child(pill(&skills.len().to_string(), t.dim, t)),
        )
        .child(div().text_xs().text_color(t.faint).child(hint));

    let body: AnyElement = if skills.is_empty() {
        card(t)
            .p_3p5()
            .text_xs()
            .text_color(t.dim)
            .child(empty_text)
            .into_any_element()
    } else {
        let count = skills.len();
        let mut cards = Vec::with_capacity(count);
        for (idx, item) in skills.iter().enumerate() {
            let path = item.path.clone();
            let expanded = daemon.expanded_skills.contains(&path);
            let name = item.name.clone();
            let desc = item.description.clone();
            let origin = item.origin.clone();
            let scope = item.scope.clone();
            let toggle_path = path.clone();
            let t_copy = *t;

            let row_head = div()
                .id(SharedString::from(format!("skill-head-{group_key}-{idx}")))
                .px_3p5()
                .py_2p5()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(move |daemon, _, _, cx| {
                    if !daemon.expanded_skills.remove(&toggle_path) {
                        daemon.expanded_skills.insert(toggle_path.clone());
                    }
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2p5()
                        .min_w_0()
                        .flex_1()
                        .child(
                            Icon::new(IconName::Blocks)
                                .with_size(Size::XSmall)
                                .text_color(t.dim),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .min_w_0()
                                .flex_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text)
                                        .truncate()
                                        .child(name),
                                )
                                .when(!desc.is_empty(), |col| {
                                    col.child(
                                        div()
                                            .text_xs()
                                            .text_color(t.dim)
                                            .truncate()
                                            .child(desc.clone()),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .flex_shrink_0()
                        .child(div().text_xs().text_color(t.faint).child(origin.clone()))
                        .child(spring_chevron(
                            SharedString::from(format!("skill-chev-{group_key}-{idx}")),
                            expanded,
                            t.faint,
                            animate_motion,
                        )),
                );

            let detail_path = path.clone();
            let detail_desc = item.description.clone();
            let reveal_body = accordion_reveal(
                SharedString::from(format!("skill-reveal-{group_key}-{idx}")),
                expanded,
                window,
                cx,
                move |_| {
                    div()
                        .px_3p5()
                        .pb_3()
                        .pt_2()
                        .border_t_1()
                        .border_color(t_copy.border_subtle)
                        .bg(t_copy.surface_subtle)
                        .flex()
                        .flex_col()
                        .gap_1p5()
                        .when(!detail_desc.is_empty(), |col| {
                            col.child(
                                div()
                                    .text_xs()
                                    .text_color(t_copy.text)
                                    .child(detail_desc),
                            )
                        })
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap_2()
                                .text_xs()
                                .text_color(t_copy.faint)
                                .child(SharedString::from(format!(
                                    "作用域：{scope} · 来源：{origin}"
                                ))),
                        )
                        .child(
                            div()
                                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                                .text_xs()
                                .text_color(t_copy.dim)
                                .child(detail_path),
                        )
                        .into_any_element()
                },
            );

            let card_el = card(t)
                .overflow_hidden()
                .child(row_head)
                .children(reveal_body)
                .into_any_element();
            cards.push(stagger_enter(
                format!("skill-stagger-{group_key}-{}-{idx}", daemon.page_epoch),
                idx,
                count,
                card_el,
                window,
                cx,
            ));
        }
        div().flex().flex_col().gap_2().children(cards).into_any_element()
    };

    let _ = &mut header;
    div()
        .flex()
        .flex_col()
        .gap_2p5()
        .child(header)
        .child(body)
        .into_any_element()
}

fn tools_mcp_body(
    daemon: &DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Vec<AnyElement> {
    let Some(config) = &daemon.config else {
        return vec![
            card(t)
                .p_4()
                .text_xs()
                .text_color(t.dim)
                .child("正在读取 daemon MCP 配置…")
                .into_any_element(),
        ];
    };

    let mut out = Vec::new();

    // 顶部概览卡
    let summary_row = div()
        .flex()
        .flex_wrap()
        .gap_3()
        .child(
            card(t)
                .flex_1()
                .min_w(px(240.))
                .p_4()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .child(Icon::new(IconName::Cable).with_size(Size::XSmall).text_color(t.dim))
                                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("MCP 客户端")),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.dim)
                                .child(SharedString::from(format!(
                                    "{} 个服务 · 空闲 {}s 回收",
                                    config.mcp.servers.len(),
                                    config.mcp.idle_shutdown_secs
                                ))),
                        ),
                )
                .child(pill(
                    if config.mcp.enabled { "已启用" } else { "已停用" },
                    if config.mcp.enabled { t.ok } else { t.dim },
                    t,
                )),
        )
        .into_any_element();
    out.push(stagger_enter(
        format!("mcp-summary-{}", daemon.page_epoch),
        0,
        3,
        summary_row,
        window,
        cx,
    ));

    // MCP servers 列表
    let mcp_total = config.mcp.servers.len();
    let mcp_cards: Vec<AnyElement> = if config.mcp.servers.is_empty() {
        vec![
            card(t)
                .p_3p5()
                .text_xs()
                .text_color(t.dim)
                .child("未配置 MCP Server（请在 ~/.qaqh/config.toml 中配置 [[mcp.servers]]）。")
                .into_any_element(),
        ]
    } else {
        config
            .mcp
            .servers
            .iter()
            .enumerate()
            .map(|(idx, server)| {
                let endpoint = if !server.command.is_empty() {
                    if server.args.is_empty() {
                        server.command.clone()
                    } else {
                        format!("{} {}", server.command, server.args.join(" "))
                    }
                } else {
                    server.url.clone()
                };
                let tools_label = match &server.tools {
                    None => "全部工具".to_owned(),
                    Some(list) => format!("{} 个工具", list.len()),
                };
                let row = card(t)
                    .p_3p5()
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
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.text)
                                            .child(server.name.clone()),
                                    )
                                    .child(pill(&server.transport, t.dim, t)),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.dim)
                                    .child(SharedString::from(format!(
                                        "{tools_label} · 超时 {}s · 并发 {}",
                                        server.default_timeout_secs, server.max_concurrent_calls
                                    ))),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .rounded(px(RADIUS_CHIP))
                            .bg(t.surface_subtle)
                            .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                            .text_xs()
                            .text_color(t.dim)
                            .truncate()
                            .child(endpoint),
                    )
                    .into_any_element();
                stagger_enter(
                    format!("mcp-srv-{}-{idx}", daemon.page_epoch),
                    idx,
                    mcp_total,
                    row,
                    window,
                    cx,
                )
            })
            .collect()
    };
    out.push(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(t.dim).child("MCP Servers"))
            .children(mcp_cards)
            .into_any_element(),
    );

    out
}

// ============================================================================
// 3. 「团队」页 (TaskBoard + Roster)
// ============================================================================

fn task_state_meta(state: TaskState, t: &Palette) -> (&'static str, Hsla) {
    match state {
        TaskState::Open => ("待认领", t.dim),
        TaskState::Claimed => ("进行中", t.accent),
        TaskState::Completed => ("已完结", t.ok),
        TaskState::Cancelled => ("已取消", t.bad),
    }
}

fn agent_status_meta(status: TeamAgentStatus, t: &Palette) -> (&'static str, Hsla, bool) {
    match status {
        TeamAgentStatus::PendingInit => ("初始化中", t.warn, true),
        TeamAgentStatus::Running => ("运行中", t.accent, true),
        TeamAgentStatus::Idle => ("空闲", t.dim, false),
        TeamAgentStatus::WaitingUser => ("等待用户", t.warn, true),
        TeamAgentStatus::Interrupted => ("已中断", t.warn, false),
        TeamAgentStatus::Completed => ("已完成", t.ok, false),
        TeamAgentStatus::Errored => ("异常", t.bad, false),
        TeamAgentStatus::Shutdown => ("已关停", t.faint, false),
        TeamAgentStatus::NotFound => ("未找到", t.bad, false),
    }
}

pub(crate) fn team_page(
    daemon: &DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> AnyElement {
    let weak = cx.entity().downgrade();
    let t_copy = *t;

    let selected_seed = daemon.team_page_seed.clone();
    let active_label = selected_seed
        .as_ref()
        .and_then(|seed| daemon.sessions.iter().find(|s| &s.id == seed))
        .map(|row| clean_session_title(row, &daemon.workspaces).to_string())
        .unwrap_or_else(|| "选择会话".to_owned());
    let active_short: String = active_label.chars().take(26).collect();

    let session_opts: Vec<(qaqh_core_vocab::SessionId, String, Option<String>)> = daemon
        .sessions
        .iter()
        .filter(|s| !s.archived)
        .map(|row| {
            let title = clean_session_title(row, &daemon.workspaces).to_string();
            let ws = row
                .workspace_id
                .as_ref()
                .and_then(|wid| daemon.workspaces.iter().find(|w| &w.id == wid))
                .map(|w| w.title.clone());
            (row.id.clone(), title, ws)
        })
        .collect();

    let session_picker = Popover::new("team-session-popover")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .trigger(popover_trigger(
            cx,
            "team-session-btn",
            IconName::MessageSquare,
            active_short,
            "切换查看目标会话的团队与任务板",
            t,
        ))
        .content(move |_state, _window, cx| {
            let popover: WeakEntity<PopoverState> = cx.entity().downgrade();
            let mut card_box = popover_card(&t_copy, 290.);
            card_box = card_box.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t_copy.faint)
                    .child("选择活跃会话"),
            );
            let mut scroll = div()
                .id("team-session-list")
                .max_h(px(280.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_0p5();
            if session_opts.is_empty() {
                scroll = scroll.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_xs()
                        .text_color(t_copy.dim)
                        .child("暂无活跃会话"),
                );
            } else {
                for (sid, title, ws) in &session_opts {
                    let is_sel = selected_seed.as_ref() == Some(sid);
                    let sid_owned = sid.clone();
                    let w = weak.clone();
                    let pop = popover.clone();
                    scroll = scroll.child(popover_option_row(
                        SharedString::from(format!("team-sess-{sid}")),
                        SharedString::from(title.clone()),
                        ws.clone().map(SharedString::from),
                        is_sel,
                        &t_copy,
                        move |window, cx| {
                            let sid = sid_owned.clone();
                            let _ = w.update(cx, |daemon, cx| {
                                daemon.team_page_seed = Some(sid);
                                daemon.page_epoch = daemon.page_epoch.wrapping_add(1);
                                daemon.refresh_team_page();
                                cx.notify();
                            });
                            let _ = pop.update(cx, |state, cx| state.dismiss(window, cx));
                        },
                    ));
                }
            }
            card_box.child(scroll)
        })
        .into_any_element();

    let refresh_btn = button(
        cx,
        BtnKind::Soft,
        "team-refresh",
        if daemon.team_page_loading {
            "刷新中…"
        } else {
            "刷新"
        },
        t,
        cx.listener(|daemon, _, _, cx| {
            daemon.refresh_team_page();
            cx.notify();
        }),
    );

    let body = team_page_body(daemon, window, cx, t);
    page_shell(
        "team-page-scroll",
        "团队与任务板",
        "跨代理任务板（认领/依赖/产出）与实时花名册投影。",
        vec![session_picker, refresh_btn.into_any_element()],
        body,
        t,
    )
    .into_any_element()
}

fn team_page_body(
    daemon: &DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> Vec<AnyElement> {
    let Some(target_seed) = &daemon.team_page_seed else {
        return vec![
            card(t)
                .p_5()
                .flex()
                .flex_col()
                .items_center()
                .gap_2()
                .child(Icon::new(IconName::Users).with_size(Size::Medium).text_color(t.faint))
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text)
                        .child("暂无可查看的团队快照"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.dim)
                        .child("请先在主页创建或选择一个会话，然后在此查看多代理协同任务与成员花名册。"),
                )
                .into_any_element(),
        ];
    };

    let snapshot = daemon
        .team_page_snapshot
        .as_ref()
        .filter(|(seed, _)| seed == target_seed)
        .map(|(_, resp)| resp);

    let Some(resp) = snapshot else {
        return vec![
            card(t)
                .p_4()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(IconName::Clock)
                        .with_size(Size::XSmall)
                        .text_color(t.dim),
                )
                .child(shimmer_label(
                    "team-loading-shimmer",
                    "正在拉取团队快照…",
                    t,
                ))
                .into_any_element(),
        ];
    };

    let tasks = &resp.tasks.tasks;
    let total_tasks = tasks.len();
    let in_progress = tasks
        .iter()
        .filter(|task| matches!(task.state, TaskState::Open | TaskState::Claimed))
        .count();
    let completed = tasks
        .iter()
        .filter(|task| matches!(task.state, TaskState::Completed))
        .count();
    let total_agents = resp.team.agents.len();

    let kpi_strip = div()
        .flex()
        .flex_wrap()
        .gap_3()
        .child(kpi_card("总任务", total_tasks.to_string(), t))
        .child(kpi_card("待办 / 进行中", in_progress.to_string(), t))
        .child(kpi_card("已完结", completed.to_string(), t))
        .child(kpi_card("团队成员", total_agents.to_string(), t))
        .into_any_element();

    let task_cards: Vec<AnyElement> = if tasks.is_empty() {
        vec![
            card(t)
                .p_4()
                .text_xs()
                .text_color(t.dim)
                .child("当前会话还没有任务板记录。")
                .into_any_element(),
        ]
    } else {
        tasks
            .iter()
            .enumerate()
            .map(|(idx, task)| {
                let (state_label, state_color) = task_state_meta(task.state, t);
                let deps_satisfied = task.depends_on.iter().all(|dep_id| {
                    tasks
                        .iter()
                        .find(|other| &other.task_id == dep_id)
                        .is_some_and(|dep| matches!(dep.state, TaskState::Completed))
                });
                let blocked = !deps_satisfied
                    && matches!(task.state, TaskState::Open | TaskState::Claimed);
                let owner_name = task
                    .owner
                    .as_deref()
                    .and_then(|o| o.rsplit('/').next())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("未认领")
                    .to_owned();

                let dep_titles: Vec<String> = task
                    .depends_on
                    .iter()
                    .map(|dep_id| {
                        tasks
                            .iter()
                            .find(|other| &other.task_id == dep_id)
                            .map(|other| other.title.clone())
                            .unwrap_or_else(|| dep_id.clone())
                    })
                    .collect();

                let mut el = card(t).p_3p5().flex().flex_col().gap_2();
                el = el.child(
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
                                .flex_1()
                                .child(pill(state_label, state_color, t))
                                .when(blocked, |row| row.child(pill("等待依赖", t.warn, t)))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(t.text)
                                        .truncate()
                                        .child(task.title.clone()),
                                ),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(t.faint)
                                .child(SharedString::from(format!(
                                    "{owner_name} · epoch {}",
                                    task.claim_epoch
                                ))),
                        ),
                );

                if !dep_titles.is_empty() {
                    el = el.child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(SharedString::from(format!("前置依赖：{}", dep_titles.join("、")))),
                    );
                }
                if !task.acceptance.is_empty() {
                    el = el.child(
                        div()
                            .pl_3()
                            .border_l_1()
                            .border_color(t.border_subtle)
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .children(task.acceptance.iter().map(|acc| {
                                div()
                                    .text_xs()
                                    .text_color(t.dim)
                                    .child(SharedString::from(format!("• {acc}")))
                            })),
                    );
                }
                if !task.write_scope_warnings.is_empty() {
                    el = el.child(
                        div()
                            .text_xs()
                            .text_color(t.warn)
                            .child(SharedString::from(format!(
                                "写冲突提示：{}",
                                task.write_scope_warnings.join(" / ")
                            ))),
                    );
                }
                if !task.artifacts.is_empty() {
                    el = el.child(
                        div()
                            .text_xs()
                            .text_color(t.faint)
                            .child(SharedString::from(format!("{} 项产出附件", task.artifacts.len()))),
                    );
                }
                stagger_enter(
                    format!("team-task-{}-{idx}", daemon.page_epoch),
                    idx,
                    total_tasks,
                    el.into_any_element(),
                    window,
                    cx,
                )
            })
            .collect()
    };

    let mut sorted_agents = resp.team.agents.clone();
    sorted_agents.sort_by(|a, b| a.agent_path.as_str().cmp(b.agent_path.as_str()));
    let agent_cards: Vec<AnyElement> = if sorted_agents.is_empty() {
        vec![
            card(t)
                .p_4()
                .text_xs()
                .text_color(t.dim)
                .child("当前会话没有子代理花名册。")
                .into_any_element(),
        ]
    } else {
        let count = sorted_agents.len();
        sorted_agents
            .into_iter()
            .enumerate()
            .map(|(idx, agent)| {
                let raw_path = agent.agent_path.as_str().to_owned();
                let depth = raw_path
                    .trim_matches('/')
                    .split('/')
                    .filter(|seg| !seg.is_empty())
                    .count()
                    .saturating_sub(1);
                let tail = raw_path
                    .trim_end_matches('/')
                    .rsplit('/')
                    .next()
                    .filter(|s| !s.is_empty())
                    .unwrap_or("root");
                let display = agent
                    .nickname
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(tail)
                    .to_owned();
                let (status_label, status_color, active) = agent_status_meta(agent.status, t);
                let residency_label = match agent.residency {
                    qaqh_ringing_v3::TeamAgentResidency::Loaded => "常驻",
                    qaqh_ringing_v3::TeamAgentResidency::Unloaded => "已休眠",
                };
                let agent_session_id = agent.agent_id.clone();
                let clickable = daemon.sessions.iter().any(|s| s.id == agent_session_id);
                let tip_path = raw_path.clone();

                let mut row = div()
                    .id(SharedString::from(format!("team-agent-{idx}")))
                    .ml(px((depth * 18) as f32))
                    .px_3p5()
                    .py_2p5()
                    .rounded(px(RADIUS_CARD))
                    .bg(t.card)
                    .border_1()
                    .border_color(t.border_subtle)
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .tooltip(move |win, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip_path.clone()).build(win, cx)
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .flex_1()
                            .child(live_dot(
                                SharedString::from(format!("team-agent-dot-{idx}")),
                                status_color,
                                active,
                            ))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text)
                                    .truncate()
                                    .child(display),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.faint)
                                    .truncate()
                                    .child(raw_path),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .flex_shrink_0()
                            .child(div().text_xs().text_color(t.faint).child(residency_label))
                            .child(pill(status_label, status_color, t)),
                    );

                if clickable {
                    row = row
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |daemon, _, window, cx| {
                            daemon.select(agent_session_id.clone(), window, cx);
                        }));
                }

                stagger_enter(
                    format!("team-agent-stagger-{}-{idx}", daemon.page_epoch),
                    idx,
                    count,
                    row.into_any_element(),
                    window,
                    cx,
                )
            })
            .collect()
    };

    vec![
        stagger_enter(
            format!("team-kpi-{}", daemon.page_epoch),
            0,
            3,
            kpi_strip,
            window,
            cx,
        ),
        div()
            .flex()
            .flex_col()
            .gap_2p5()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("任务板"))
            .children(task_cards)
            .into_any_element(),
        div()
            .flex()
            .flex_col()
            .gap_2p5()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(t.text).child("成员花名册"))
            .children(agent_cards)
            .into_any_element(),
    ]
}

fn kpi_card(label: &'static str, value: String, t: &Palette) -> Div {
    card(t)
        .flex_1()
        .min_w(px(150.))
        .px_4()
        .py_3()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(t.dim).child(label))
        .child(
            div()
                .text_lg()
                .font_weight(FontWeight::BOLD)
                .text_color(t.text)
                .child(value),
        )
}

// ============================================================================
// 4. 「接入」页 (模型目录概览 + OAuth/套餐账号额度管理)
// ============================================================================

pub(crate) fn access_page(
    daemon: &DaemonStore,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
    t: &Palette,
) -> AnyElement {
    let refresh_btn = button(
        cx,
        BtnKind::Soft,
        "access-refresh",
        "刷新账号状态",
        t,
        cx.listener(|daemon, _, _, cx| {
            daemon.refresh_accounts();
            let _ = daemon
                .requests
                .unbounded_send(BridgeRequest::Config(qaqh_ringing_v3::ConfigRequest::ListProviders));
            cx.notify();
        }),
    );
    let catalog_btn = button(
        cx,
        BtnKind::Primary,
        "access-open-catalog",
        "管理服务商与模型",
        t,
        cx.listener(|daemon, _, window, cx| {
            daemon.open_settings(SettingsEditor::Catalog, window, cx);
        }),
    );

    let (default_model_text, provider_count, model_count) = match &daemon.providers {
        Some(catalog) => {
            let def = catalog
                .default_model_selection
                .as_ref()
                .map(|m| format!("{} / {}", m.provider_id, m.model_id))
                .unwrap_or_else(|| "尚未设定默认模型".to_owned());
            let mut active_providers = 0usize;
            let mut active_models = 0usize;
            for (pid, provider) in &catalog.providers {
                let mut has_any = false;
                for mid in provider.models.keys() {
                    if qaqh_profile::resolve_model_selection(
                        catalog,
                        &qaqh_profile::ModelSelection {
                            provider_id: pid.clone(),
                            model_id: mid.clone(),
                            options: Default::default(),
                        },
                    )
                    .is_ok()
                    {
                        has_any = true;
                        active_models += 1;
                    }
                }
                if has_any {
                    active_providers += 1;
                }
            }
            (def, active_providers, active_models)
        }
        None => ("读取模型目录中…".to_owned(), 0, 0),
    };

    let overview = stagger_enter(
        format!("access-overview-{}", daemon.page_epoch),
        0,
        2,
        card(t)
            .p_4()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(Icon::new(IconName::KeyRound).with_size(Size::XSmall).text_color(t.dim))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.text)
                                    .child("当前模型目录与默认来源"),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(SharedString::from(format!(
                                "全局默认：{default_model_text} · 已配置 {provider_count} 个服务商 / {model_count} 个可用模型"
                            ))),
                    ),
            )
            .child(button(
                cx,
                BtnKind::Ghost,
                "access-edit-byok",
                "管理服务商目录 →",
                t,
                cx.listener(|daemon, _, window, cx| {
                    daemon.open_settings(SettingsEditor::Catalog, window, cx);
                }),
            ))
            .into_any_element(),
        window,
        cx,
    );

    let accounts_el = stagger_enter(
        format!("access-accounts-{}", daemon.page_epoch),
        1,
        2,
        crate::settings_ui::accounts(daemon, cx, t).into_any_element(),
        window,
        cx,
    );

    page_shell(
        "access-page-scroll",
        "账号与接入",
        "统一管理订阅套餐浏览器授权与自带 API Key（BYOK）服务商目录。",
        vec![refresh_btn.into_any_element(), catalog_btn.into_any_element()],
        vec![overview, accounts_el],
        t,
    )
    .into_any_element()
}
