//! 悬浮居中圆角 Composer(DESIGN LAW v2.0):
//! - 居中圆角岛(18px 圆角 + 柔和浮影 + 运行态轻光晕);
//! - 无框自适应多行 Textarea;
//! - 模型 / 推理强度 / 工具模式全走可见 Popover 下拉列表(告别盲点循环),零学习成本。

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Anchor, ClickEvent, Div, FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton,
    ParentElement, SharedString, Styled, WeakEntity, Window, div, px, relative,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{Disableable, Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{
    BtnKind, RADIUS_CARD, RADIUS_CHIP, RADIUS_COMPOSER, RADIUS_CONTROL, RADIUS_POPOVER, button,
    pill, shadow_composer, shadow_popover, soft,
};
use qaqh_gpui_kit::primitives::{icon_button, segmented_button, setting_row};
use qaqh_gpui_kit::theme::Palette;
use qaqh_gpui_wire::delivery::DeliveryState;
use qaqh_ringing_contract::fact::ActivityState;
use qaqh_types::ModelSelection;

use crate::store::SessionStore;

fn hover_shift(color: Hsla, t: &Palette, delta: f32) -> Hsla {
    let step = if t.bg.l > 0.5 { -delta } else { delta };
    Hsla {
        l: (color.l + step).clamp(0., 1.),
        ..color
    }
}

fn token_badge(tokens: u32) -> String {
    if tokens >= 1_000_000 {
        format!("{}M", tokens / 1_000_000)
    } else {
        format!("{}k", tokens / 1_000)
    }
}

fn format_tokens_compact(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let tenths = (tokens as f64 / 100_000.0).round() as u64;
        if tenths.is_multiple_of(10) {
            format!("{}M", tenths / 10)
        } else {
            format!("{:.1}M", tenths as f64 / 10.0)
        }
    } else if tokens >= 1_000 {
        let tenths = (tokens as f64 / 100.0).round() as u64;
        if tenths.is_multiple_of(10) {
            format!("{}k", tenths / 10)
        } else {
            format!("{:.1}k", tenths as f64 / 10.0)
        }
    } else {
        tokens.to_string()
    }
}

fn format_tokens_exact(tokens: u64) -> String {
    let digits: Vec<char> = tokens.to_string().chars().collect();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (idx, ch) in digits.iter().enumerate() {
        if idx > 0 && (digits.len() - idx).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*ch);
    }
    out
}

fn telemetry_footer(store: &SessionStore, t: &Palette) -> Option<AnyElement> {
    let tel = store.telemetry()?;

    let ctx_color = if tel.used_pct.is_some_and(|pct| pct >= 90) {
        t.bad
    } else if tel.used_pct.is_some_and(|pct| pct >= 75) {
        t.warn
    } else {
        t.dim
    };
    let bar_fill = if tel.used_pct.is_some_and(|pct| pct >= 90) {
        t.bad
    } else if tel.used_pct.is_some_and(|pct| pct >= 75) {
        t.warn
    } else {
        t.text
    };

    let used_compact = format_tokens_compact(u64::from(tel.used_tokens));
    let ctx_label = match (tel.context_window, tel.used_pct) {
        (Some(window), Some(pct)) => format!(
            "{pct}% · {used_compact} / {}",
            format_tokens_compact(u64::from(window))
        ),
        _ => format!("{used_compact} ctx"),
    };
    let used_exact = format_tokens_exact(u64::from(tel.used_tokens));
    let mut ctx_tip = match (tel.context_window, tel.used_pct) {
        (Some(window), Some(pct)) => format!(
            "当前上下文占用: {used_exact} / {} tokens ({pct}%)",
            format_tokens_exact(u64::from(window))
        ),
        _ => format!("当前上下文占用: {used_exact} tokens (模型窗口未知)"),
    };
    ctx_tip.push_str(&format!(
        "\n最近请求: 输入 {} · 输出 {}",
        format_tokens_exact(u64::from(tel.prompt_tokens)),
        format_tokens_exact(u64::from(tel.completion_tokens)),
    ));
    if tel.reasoning_tokens > 0 {
        ctx_tip.push_str(&format!(
            " · 思考 {}",
            format_tokens_exact(u64::from(tel.reasoning_tokens))
        ));
    }
    let ctx_tip: SharedString = ctx_tip.into();

    let (cache_color, cache_label, mut cache_tip) = match &tel.latest_cache {
        Some(stats) => {
            let color = if stats.hit_pct >= 60 {
                t.ok
            } else if stats.hit_pct > 0 {
                t.dim
            } else {
                t.warn
            };
            let label = format!(
                "缓存 {}% · {}",
                stats.hit_pct,
                format_tokens_compact(stats.hit_tokens)
            );
            let tip = format!(
                "最近请求缓存命中: {}% (命中 {} / {} tokens)",
                stats.hit_pct,
                format_tokens_exact(stats.hit_tokens),
                format_tokens_exact(stats.total_tokens),
            );
            (color, label, tip)
        }
        None => (
            t.faint,
            "缓存 —".to_owned(),
            "最近请求缓存命中: 服务商未上报缓存统计".to_owned(),
        ),
    };
    if tel.usage_requests > 1
        && let Some(cum) = &tel.cumulative_cache
    {
        cache_tip.push_str(&format!(
            "\n本会话累计命中: {}% (命中 {} / {} · 共 {} 次请求)",
            cum.hit_pct,
            format_tokens_exact(cum.hit_tokens),
            format_tokens_exact(cum.total_tokens),
            tel.usage_requests,
        ));
    }
    let cache_tip: SharedString = cache_tip.into();

    let ctx_meter = div()
        .id("composer-ctx-meter")
        .flex()
        .items_center()
        .gap_1p5()
        .min_w_0()
        .tooltip(move |window, cx| Tooltip::new(ctx_tip.clone()).build(window, cx))
        .child(
            Icon::new(IconName::Gauge)
                .with_size(Size::XSmall)
                .text_color(ctx_color),
        )
        .children(tel.used_pct.map(|pct| {
            let ratio = (f32::from(pct) / 100.0).clamp(0.0, 1.0);
            div()
                .w(px(44.))
                .h(px(4.))
                .rounded_full()
                .bg(soft(t.border, 0.7))
                .overflow_hidden()
                .child(div().h_full().w(relative(ratio)).rounded_full().bg(bar_fill))
        }))
        .child(
            div()
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_size(px(10.))
                .text_color(ctx_color)
                .truncate()
                .child(SharedString::from(ctx_label)),
        );

    let cache_meter = div()
        .id("composer-cache-meter")
        .flex()
        .items_center()
        .gap_1p5()
        .flex_shrink_0()
        .tooltip(move |window, cx| Tooltip::new(cache_tip.clone()).build(window, cx))
        .child(
            Icon::new(IconName::DatabaseZap)
                .with_size(Size::XSmall)
                .text_color(cache_color),
        )
        .child(
            div()
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_size(px(10.))
                .text_color(cache_color)
                .child(SharedString::from(cache_label)),
        );

    Some(
        div()
            .id("composer-telemetry-footer")
            .h(px(26.))
            .px_3()
            .bg(t.surface_subtle)
            .border_t_1()
            .border_color(t.border_subtle)
            .rounded_b(px(RADIUS_COMPOSER - 1.0))
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(ctx_meter)
            .child(cache_meter)
            .into_any_element(),
    )
}

fn tools_mode_label(mode: &str) -> &'static str {
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

fn solid_round_button(
    cx: &gpui_kit::App,
    id: &'static str,
    icon: IconName,
    bg: Hsla,
    fg: Hsla,
    hover: Hsla,
    tooltip: &'static str,
    disabled: bool,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let button = Button::new(id)
        .with_size(Size::Medium)
        .rounded(ButtonRounded::Size(px(999.)))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(bg)
                .foreground(fg)
                .hover(hover)
                .active(hover),
        )
        .icon(Icon::new(icon).text_color(fg))
        .tooltip(tooltip)
        .on_click(listener);
    let button = if disabled { button.disabled(true) } else { button };
    button.into_any_element()
}

fn status_icon(icon: IconName, color: Hsla) -> AnyElement {
    div()
        .flex()
        .items_center()
        .px_1()
        .child(Icon::new(icon).with_size(Size::Small).text_color(color))
        .into_any_element()
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

fn model_popover(
    store: &SessionStore,
    entity: WeakEntity<SessionStore>,
    cx: &gpui_kit::App,
    t: &Palette,
) -> AnyElement {
    let current = store
        .settings
        .as_ref()
        .and_then(|s| s.meta.model_selection.as_ref())
        .cloned();
    let label = match (&store.catalog, &current) {
        (Some(catalog), Some(sel)) => catalog
            .providers
            .get(&sel.provider_id)
            .and_then(|p| p.models.get(&sel.model_id))
            .and_then(|m| m.display_name.as_deref())
            .filter(|name| !name.is_empty())
            .map(|name| name.to_owned())
            .unwrap_or_else(|| sel.model_id.clone()),
        (_, Some(sel)) => sel.model_id.clone(),
        (_, None) => "选择模型".to_owned(),
    };
    let short_label: String = label.chars().take(24).collect();
    let trigger = popover_trigger(
        cx,
        "composer-model-btn",
        IconName::Cpu,
        short_label,
        "切换当前会话模型",
        t,
    );

    #[derive(Clone)]
    struct ModelGroup {
        provider_name: String,
        items: Vec<(ModelSelection, String, String)>,
    }
    let groups: Vec<ModelGroup> = store
        .catalog
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
                        if qaqh_profile::resolve_model_selection(catalog, &selection).is_ok() {
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
                    (!items.is_empty()).then(|| ModelGroup {
                        provider_name: if provider.name.is_empty() {
                            pid.clone()
                        } else {
                            provider.name.clone()
                        },
                        items,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let t = *t;
    Popover::new("composer-model-popover")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .trigger(trigger)
        .content(move |_state, _window, cx| {
            let popover: WeakEntity<PopoverState> = cx.entity().downgrade();
            let mut card = popover_card(&t, 280.);
            let manage_row = {
                let entity = entity.clone();
                let popover = popover.clone();
                div()
                    .pt_1()
                    .mt_1()
                    .border_t_1()
                    .border_color(t.border_subtle)
                    .child(
                        div()
                            .id("composer-model-manage")
                            .px_2()
                            .py_1p5()
                            .rounded(px(RADIUS_CHIP))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(t.dim)
                            .hover(move |s| s.bg(t.hover).text_color(t.text))
                            .on_click(move |_, window, cx| {
                                let _ = popover.update(cx, |state, cx| state.dismiss(window, cx));
                                let _ = entity.update(cx, |store, cx| {
                                    store.open_model_params(window, cx);
                                });
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_1p5()
                                    .child(
                                        Icon::new(IconName::SlidersHorizontal)
                                            .with_size(Size::XSmall)
                                            .text_color(t.dim),
                                    )
                                    .child("管理模型参数与策略"),
                            )
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .with_size(Size::XSmall)
                                    .text_color(t.faint),
                            ),
                    )
            };

            if groups.is_empty() {
                return card
                    .child(
                        div()
                            .px_2()
                            .py_2()
                            .text_xs()
                            .text_color(t.dim)
                            .child("暂无可用模型，请先在设置中配置服务商。"),
                    )
                    .child(manage_row);
            }
            let mut list = div()
                .id("composer-model-list")
                .max_h(px(300.))
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .gap_1();
            for group in &groups {
                list = list
                    .child(
                        div()
                            .px_2()
                            .pt_1()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.faint)
                            .child(group.provider_name.clone()),
                    )
                    .children(group.items.iter().map(|(selection, display, ctx)| {
                        let is_selected = current.as_ref().is_some_and(|cur| {
                            cur.provider_id == selection.provider_id
                                && cur.model_id == selection.model_id
                        });
                        let sel = selection.clone();
                        let entity = entity.clone();
                        let popover = popover.clone();
                        popover_option_row(
                            SharedString::from(format!(
                                "pick-model-{}-{}",
                                sel.provider_id, sel.model_id
                            )),
                            SharedString::from(display.clone()),
                            Some(SharedString::from(ctx.clone())),
                            is_selected,
                            &t,
                            move |window, cx| {
                                let sel = sel.clone();
                                let _ = entity.update(cx, |store, cx| {
                                    store.patch_model(sel);
                                    cx.notify();
                                });
                                let _ = popover.update(cx, |state, cx| state.dismiss(window, cx));
                            },
                        )
                    }));
            }
            card = card.child(list).child(manage_row);
            card
        })
        .into_any_element()
}

fn effort_popover(
    store: &SessionStore,
    entity: WeakEntity<SessionStore>,
    cx: &gpui_kit::App,
    t: &Palette,
) -> AnyElement {
    let model = store
        .settings
        .as_ref()
        .and_then(|s| s.meta.model_selection.as_ref());
    let current_effort = model.and_then(|m| m.options.reasoning_effort.clone());
    let label = current_effort.as_deref().unwrap_or("默认").to_owned();
    let levels: Vec<String> = store
        .catalog
        .as_deref()
        .zip(model)
        .map(|(catalog, m)| crate::store::model_effort_levels(catalog, m))
        .unwrap_or_default();

    let trigger = popover_trigger(
        cx,
        "composer-effort-btn",
        IconName::Gauge,
        label,
        "选择推理思考深度",
        t,
    );
    let t = *t;
    Popover::new("composer-effort-popover")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .trigger(trigger)
        .content(move |_state, _window, cx| {
            let popover = cx.entity().downgrade();
            let mut card = popover_card(&t, 180.);
            card = card.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .child("推理强度"),
            );
            let mut options: Vec<(Option<String>, String)> = vec![(None, "默认".to_owned())];
            for level in &levels {
                options.push((Some(level.clone()), level.clone()));
            }
            for (val, text) in options {
                let is_selected = current_effort == val;
                let entity = entity.clone();
                let popover = popover.clone();
                card = card.child(popover_option_row(
                    SharedString::from(format!("pick-effort-{}", text)),
                    SharedString::from(text),
                    None,
                    is_selected,
                    &t,
                    move |window, cx| {
                        let val = val.clone();
                        let _ = entity.update(cx, |store, cx| {
                            store.patch_effort(val);
                            cx.notify();
                        });
                        let _ = popover.update(cx, |state, cx| state.dismiss(window, cx));
                    },
                ));
            }
            card
        })
        .into_any_element()
}

fn tools_popover(
    store: &SessionStore,
    entity: WeakEntity<SessionStore>,
    cx: &gpui_kit::App,
    t: &Palette,
) -> AnyElement {
    let current = store
        .settings
        .as_ref()
        .map(|s| s.meta.tool_mode.clone())
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| qaqh_types::STANDARD.to_owned());
    let trigger = popover_trigger(
        cx,
        "composer-tools-btn",
        IconName::Wrench,
        tools_mode_label(&current),
        "切换会话工具权限预设",
        t,
    );
    let t = *t;
    Popover::new("composer-tools-popover")
        .anchor(Anchor::BottomLeft)
        .appearance(false)
        .trigger(trigger)
        .content(move |_state, _window, cx| {
            let popover = cx.entity().downgrade();
            let presets = [
                (qaqh_types::STANDARD, "标准工具", "完整工程读写、搜索与终端"),
                (qaqh_types::MINIMAL, "精简", "基础文件读写与命令执行"),
                (qaqh_types::MINIMAL_B, "精简 B", "轻量只读检查与检索"),
                (qaqh_types::MINIMAL_C, "精简 C", "最小对话工具集"),
            ];
            let mut card = popover_card(&t, 224.);
            card = card.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.faint)
                    .child("工具权限预设"),
            );
            for (preset, title, desc) in presets {
                let is_selected = current == preset;
                let entity = entity.clone();
                let popover = popover.clone();
                card = card.child(popover_option_row(
                    SharedString::from(format!("pick-tools-{preset}")),
                    SharedString::from(title),
                    Some(SharedString::from(desc)),
                    is_selected,
                    &t,
                    move |window, cx| {
                        let _ = entity.update(cx, |store, cx| {
                            store.patch_tools_mode(preset.to_string());
                            cx.notify();
                        });
                        let _ = popover.update(cx, |state, cx| state.dismiss(window, cx));
                    },
                ));
            }
            card
        })
        .into_any_element()
}

/// Composer 模型选择器「管理」弹出的居中参数与策略卡片。
pub fn model_params_modal(
    store: &SessionStore,
    cx: &mut Context<SessionStore>,
    t: &Palette,
) -> Option<AnyElement> {
    if !store.model_params_open {
        return None;
    }

    let settings_meta = store.settings.as_ref().map(|s| &s.meta);
    let current_model = settings_meta.and_then(|m| m.model_selection.as_ref());
    let model_def = store
        .catalog
        .as_deref()
        .zip(current_model)
        .and_then(|(catalog, sel)| {
            catalog
                .providers
                .get(&sel.provider_id)?
                .models
                .get(&sel.model_id)
        });

    let model_text = current_model
        .map(|m| format!("{} / {}", m.provider_id, m.model_id))
        .unwrap_or_else(|| "继承全局默认".to_owned());
    let ctx_badge = model_def.map(|m| format!("{} ctx", token_badge(m.context_window)));

    let mut rows: Vec<AnyElement> = Vec::new();

    // 1. 当前模型信息
    let model_value = div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(
            div()
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_xs()
                .text_color(t.text)
                .truncate()
                .max_w(px(240.))
                .child(model_text),
        )
        .children(ctx_badge.map(|badge| pill(&badge, t.dim, t)))
        .into_any_element();
    rows.push(setting_row("当前模型", "", model_value, t).into_any_element());

    // 2. 推理深度
    if let Some(sel) = current_model {
        let levels = store
            .catalog
            .as_deref()
            .map(|catalog| crate::store::model_effort_levels(catalog, sel))
            .unwrap_or_default();
        if !levels.is_empty() {
            let current_effort = sel.options.reasoning_effort.as_deref();
            let effort_opts: Vec<(Option<String>, String)> =
                std::iter::once((None, "默认".to_owned()))
                    .chain(levels.into_iter().map(|l| (Some(l.clone()), l)))
                    .collect();
            let effort_btns = effort_opts.into_iter().map(|(val, label)| {
                let selected = current_effort == val.as_deref();
                segmented_button(
                    format!("mp-effort-{label}"),
                    label,
                    selected,
                    t,
                    cx.listener(move |store, _, _, cx| {
                        store.patch_effort(val.clone());
                        cx.notify();
                    }),
                )
            });
            rows.push(
                setting_row(
                    "推理深度",
                    "",
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1()
                        .children(effort_btns)
                        .into_any_element(),
                    t,
                )
                .into_any_element(),
            );
        }

        // 3. 最大输出 Token
        if let Some(max_out_input) = &store.model_params_max_output {
            let input_apply = max_out_input.clone();
            let input_enter = max_out_input.clone();
            let max_out_ctrl = div()
                .flex()
                .items_center()
                .gap_1p5()
                .on_key_down(cx.listener(move |store, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "enter" {
                        let raw = input_enter.read(cx).value().trim().to_string();
                        if raw.is_empty() {
                            store.patch_max_output_tokens(None);
                        } else if let Ok(v) = raw.parse::<u32>()
                            && v > 0
                        {
                            store.patch_max_output_tokens(Some(v));
                        } else {
                            store.operation_error = Some("最大输出 Token 必须为正整数".into());
                        }
                        cx.notify();
                    }
                }))
                .child(div().w(px(120.)).child(Input::new(max_out_input).small()))
                .child(button(
                    cx,
                    BtnKind::Soft,
                    "mp-apply-max-output",
                    "应用",
                    t,
                    cx.listener(move |store, _, _, cx| {
                        let raw = input_apply.read(cx).value().trim().to_string();
                        if raw.is_empty() {
                            store.patch_max_output_tokens(None);
                        } else if let Ok(v) = raw.parse::<u32>()
                            && v > 0
                        {
                            store.patch_max_output_tokens(Some(v));
                        } else {
                            store.operation_error = Some("最大输出 Token 必须为正整数".into());
                        }
                        cx.notify();
                    }),
                ))
                .into_any_element();
            rows.push(setting_row("最大输出 Token", "留空使用模型默认", max_out_ctrl, t).into_any_element());
        }

        // 4. 原生网页搜索(若模型支持)
        if model_def.is_some_and(|m| m.capabilities.native_web_search) {
            let cur_search = sel.options.native_web_search;
            let search_btns = [
                (None, "默认", "mp-web-default"),
                (Some(true), "开启", "mp-web-on"),
                (Some(false), "关闭", "mp-web-off"),
            ]
            .into_iter()
            .map(|(val, label, id)| {
                segmented_button(
                    id.to_owned(),
                    label,
                    cur_search == val,
                    t,
                    cx.listener(move |store, _, _, cx| {
                        store.patch_native_web_search(val);
                        cx.notify();
                    }),
                )
            });
            rows.push(
                setting_row(
                    "原生网页搜索",
                    "",
                    div()
                        .flex()
                        .gap_1()
                        .children(search_btns)
                        .into_any_element(),
                    t,
                )
                .into_any_element(),
            );
        }
    }

    // 5. 工具策略预设
    let current_tool_mode = settings_meta
        .map(|m| m.tool_mode.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or(qaqh_types::STANDARD);
    let tool_btns = [
        (qaqh_types::STANDARD, "标准"),
        (qaqh_types::MINIMAL, "精简"),
        (qaqh_types::MINIMAL_B, "精简 B"),
        (qaqh_types::MINIMAL_C, "精简 C"),
    ]
    .into_iter()
    .map(|(preset, label)| {
        segmented_button(
            format!("mp-tools-{preset}"),
            label,
            current_tool_mode == preset,
            t,
            cx.listener(move |store, _, _, cx| {
                store.patch_tools_mode(preset.to_owned());
                cx.notify();
            }),
        )
    });
    rows.push(
        setting_row(
            "工具策略",
            if current_tool_mode == qaqh_types::CUSTOM {
                "当前使用自定义工具名单"
            } else {
                ""
            },
            div()
                .flex()
                .flex_wrap()
                .gap_1()
                .children(tool_btns)
                .into_any_element(),
            t,
        )
        .into_any_element(),
    );

    // 6. 自定义工具名单
    if let Some(custom_input) = &store.model_params_custom_tools {
        let input_apply = custom_input.clone();
        let input_enter = custom_input.clone();
        let custom_ctrl = div()
            .flex()
            .items_center()
            .gap_1p5()
            .on_key_down(cx.listener(move |store, event: &KeyDownEvent, _, cx| {
                if event.keystroke.key == "enter" {
                    let names: Vec<String> = input_enter
                        .read(cx)
                        .value()
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    if !names.is_empty() {
                        store.patch_custom_tools(names);
                        cx.notify();
                    }
                }
            }))
            .child(div().w(px(180.)).child(Input::new(custom_input).small()))
            .child(button(
                cx,
                BtnKind::Soft,
                "mp-apply-custom-tools",
                "应用",
                t,
                cx.listener(move |store, _, _, cx| {
                    let names: Vec<String> = input_apply
                        .read(cx)
                        .value()
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect();
                    if !names.is_empty() {
                        store.patch_custom_tools(names);
                        cx.notify();
                    }
                }),
            ))
            .into_any_element();
        rows.push(setting_row("自定义工具", "英文逗号分隔", custom_ctrl, t).into_any_element());
    }

    let modal_card = div()
        .id("model-params-modal-card")
        .w(px(520.))
        .max_w(gpui_kit::relative(0.92))
        .rounded(px(RADIUS_CARD))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .shadow(shadow_popover(t))
        .overflow_hidden()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(|_, _, cx| cx.stop_propagation())
        .on_key_down(cx.listener(|store, event: &KeyDownEvent, _, cx| {
            if event.keystroke.key == "escape" {
                store.model_params_open = false;
                cx.notify();
                cx.stop_propagation();
            }
        }))
        .flex()
        .flex_col()
        .child(
            div()
                .px_4()
                .py_3()
                .bg(t.surface_subtle)
                .border_b_1()
                .border_color(t.border_subtle)
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            Icon::new(IconName::SlidersHorizontal)
                                .with_size(Size::XSmall)
                                .text_color(t.accent),
                        )
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child("模型参数与策略"),
                        ),
                )
                .child(
                    div()
                        .id("close-model-params")
                        .size(px(24.))
                        .rounded(px(RADIUS_CHIP))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(|store, _, _, cx| {
                            store.model_params_open = false;
                            cx.notify();
                        }))
                        .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.dim)),
                ),
        )
        .child(div().px_4().pb_2().flex().flex_col().children(rows));

    Some(
        div()
            .id("model-params-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .bg(t.scrim)
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .on_click(cx.listener(|store, _, _, cx| {
                store.model_params_open = false;
                cx.notify();
            }))
            .child(modal_card)
            .into_any_element(),
    )
}

pub fn render(
    store: &SessionStore,
    cx: &mut Context<SessionStore>,
    t: &Palette,
    content_width: f32,
) -> Div {
    // ADR-016:封箱会话 loop 已闭门,置灰禁用输入并提供后继会话直达入口。
    if let Some(seal) = store.handoff_seal.as_ref() {
        let summary = seal.work_summary.trim();
        let subtitle = if summary.is_empty() {
            "上下文已封箱并移交，无法继续在此会话输入".to_owned()
        } else {
            format!("已移交至后继会话 · {summary}")
        };
        return div()
            .w_full()
            .max_w(px(content_width + 48.))
            .mx_auto()
            .px_4()
            .pb_3()
            .child(
                div()
                    .rounded(px(RADIUS_COMPOSER))
                    .bg(t.surface_subtle)
                    .border_1()
                    .border_color(t.border)
                    .px_4()
                    .py_3()
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
                            .gap_2p5()
                            .child(
                                Icon::new(IconName::PackageCheck)
                                    .with_size(Size::Small)
                                    .text_color(t.ok),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.text)
                                            .child("当前会话已封箱交接"),
                                    )
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(t.dim)
                                            .child(subtitle),
                                    ),
                            ),
                    )
                    .child(
                        Button::new("composer-handoff-open")
                            .with_size(Size::Small)
                            .rounded(ButtonRounded::Size(px(RADIUS_CHIP)))
                            .custom(
                                ButtonCustomVariant::new(cx)
                                    .color(t.accent)
                                    .foreground(t.on_accent)
                                    .hover(t.accent_hover)
                                    .active(t.accent_hover),
                            )
                            .icon(Icon::new(IconName::ArrowRight).text_color(t.on_accent))
                            .label("打开后继会话")
                            .on_click(cx.listener(|store, _, _, cx| {
                                store.open_handoff_successor(cx);
                            })),
                    ),
            );
    }

    let running = store.activity == Some(ActivityState::Running);
    let blocked = (!running).then(|| store.send_block_reason()).flatten();
    let entity = cx.entity().downgrade();

    // 素材行:图片缩略(角标 ×)+ 文本引用 chip(尾部 ×)。
    let strip = (!store.images.is_empty() || !store.attachments.is_empty()).then(|| {
        div()
            .flex()
            .flex_wrap()
            .gap_1p5()
            .px_1()
            .pb_1p5()
            .children(store.images.iter().enumerate().map(|(index, block)| {
                image_chip(cx, index, block, t)
            }))
            .children(
                store
                    .attachments
                    .iter()
                    .enumerate()
                    .map(|(index, reference)| file_chip(cx, index, reference, t)),
            )
    });

    // 工具行:左 = 附件入口 + 发送回执态;右 = 可视化 Popover 选择器 + 主发送/停止圆钮。
    let mut toolbar = div().flex().items_center().gap_1p5().px_1().pt_0p5();
    if store.attachment_pending {
        toolbar = toolbar.child(status_icon(IconName::LoaderCircle, t.accent));
    } else {
        toolbar = toolbar.child(icon_button(
            cx,
            "composer-attach",
            IconName::Plus,
            t.dim,
            "添加图片或文本附件",
            cx.listener(|store, _, _, cx| {
                if store.attachment_pending {
                    return;
                }
                let picker = cx.prompt_for_paths(gpui_kit::PathPromptOptions {
                    files: true,
                    directories: false,
                    multiple: false,
                    prompt: Some("选择图片或 UTF-8 文本（最大 4 MiB）".into()),
                });
                cx.spawn(async move |entity, cx| {
                    if let Ok(Ok(Some(paths))) = picker.await
                        && let Some(path) = paths.into_iter().next()
                    {
                        let _ = entity.update(cx, |store, cx| {
                            store.attachment_pending = true;
                            if store
                                .requests()
                                .unbounded_send(qaqh_gpui_wire::bridge::BridgeRequest::AttachFile {
                                    seed: store.session_id.clone(),
                                    path,
                                })
                                .is_err()
                            {
                                store.attachment_pending = false;
                                store.operation_error = Some("连接已关闭".into());
                            }
                            cx.notify();
                        });
                    }
                })
                .detach();
            }),
            t,
        ));
    }
    if let Some(outbox) = &store.outbox {
        match outbox.state {
            DeliveryState::Sending => {
                toolbar = toolbar.child(status_icon(IconName::LoaderCircle, t.accent));
            }
            DeliveryState::Accepted => {
                toolbar = toolbar.child(icon_button(
                    cx,
                    "composer-receipt",
                    IconName::ReceiptText,
                    t.accent,
                    "查回执",
                    cx.listener(|store, _, _, _| store.check_receipt()),
                    t,
                ));
            }
            DeliveryState::Succeeded => {
                toolbar = toolbar.child(status_icon(IconName::CircleCheck, t.ok));
            }
            DeliveryState::Rejected | DeliveryState::Failed => {
                toolbar = toolbar.child(icon_button(
                    cx,
                    "composer-restore",
                    IconName::Undo2,
                    t.warn,
                    "恢复为草稿",
                    cx.listener(|store, _, window, cx| {
                        store.restore_outbox(window, cx);
                        cx.notify();
                    }),
                    t,
                ));
            }
            DeliveryState::Uncertain => {
                toolbar = toolbar
                    .child(icon_button(
                        cx,
                        "composer-receipt",
                        IconName::ReceiptText,
                        t.accent,
                        "查回执",
                        cx.listener(|store, _, _, _| store.check_receipt()),
                        t,
                    ))
                    .child(icon_button(
                        cx,
                        "composer-retry",
                        IconName::RotateCcw,
                        t.warn,
                        "重试原请求",
                        cx.listener(|store, _, _, cx| {
                            store.retry_message();
                            cx.notify();
                        }),
                        t,
                    ));
            }
        }
    }
    if let Some(error) = store.operation_error.clone() {
        toolbar = toolbar.child(icon_button(
            cx,
            "composer-error",
            IconName::CircleAlert,
            t.bad,
            error,
            cx.listener(|store, _, _, cx| {
                store.operation_error = None;
                cx.notify();
            }),
            t,
        ));
    }

    toolbar = toolbar
        .child(div().flex_1())
        .child(model_popover(store, entity.clone(), cx, t))
        .child(effort_popover(store, entity.clone(), cx, t))
        .child(tools_popover(store, entity, cx, t))
        .children(store.workspace_tail().map(|tail| {
            div()
                .px_2()
                .py_0p5()
                .rounded(px(RADIUS_CHIP))
                .bg(t.surface_subtle)
                .flex()
                .items_center()
                .gap_1()
                .text_xs()
                .text_color(t.dim)
                .child(Icon::new(IconName::Folder).with_size(Size::XSmall).text_color(t.dim))
                .child(tail)
                .into_any_element()
        }));

    if running {
        toolbar = toolbar.child(solid_round_button(
            cx,
            "composer-stop",
            IconName::Square,
            t.bad,
            t.on_accent,
            hover_shift(t.bad, t, 0.06),
            "停止当前生成",
            false,
            cx.listener(|store, _, _, cx| {
                store.cancel_turn();
                store.arm_cancel_watchdog(5000);
                cx.notify();
            }),
        ));
    } else {
        if let Some(reason) = blocked {
            toolbar = toolbar.child(icon_button(
                cx,
                "composer-blocked",
                IconName::CircleAlert,
                t.warn,
                reason,
                cx.listener(|_, _, _, _| {}),
                t,
            ));
        }
        toolbar = toolbar.child(solid_round_button(
            cx,
            "composer-send",
            IconName::ArrowUp,
            t.accent,
            t.on_accent,
            t.accent_hover,
            "发送消息 (Enter)",
            blocked.is_some(),
            cx.listener(|store, _, _, cx| {
                store.composer_send(cx);
                cx.notify();
            }),
        ));
    }

    div()
        .w_full()
        .max_w(px(content_width + 48.))
        .mx_auto()
        .px_4()
        .pb_3()
        .child(
            div()
                .flex()
                .flex_col()
                .rounded(px(RADIUS_COMPOSER))
                .bg(t.card)
                .border_1()
                .border_color(if running {
                    soft(t.accent, 0.45)
                } else {
                    t.border
                })
                .shadow(shadow_composer(t, running))
                .child(
                    div()
                        .px_2p5()
                        .pt_2()
                        .pb_1p5()
                        .flex()
                        .flex_col()
                        .child(Textarea::new(&store.composer).appearance(false).bordered(false))
                        .children(strip)
                        .child(toolbar),
                )
                .children(telemetry_footer(store, t)),
        )
}

fn image_chip(
    cx: &mut Context<SessionStore>,
    index: usize,
    block: &qaqh_client::ImageBlock,
    t: &Palette,
) -> AnyElement {
    div()
        .relative()
        .child(qaqh_gpui_render::media::thumb(block))
        .child(
            div()
                .id(SharedString::from(format!("composer-image-remove-{index}")))
                .absolute()
                .top(px(-4.))
                .right(px(-4.))
                .size(px(16.))
                .rounded_full()
                .bg(t.bad)
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|style| style.bg(hover_shift(t.bad, t, 0.06)))
                .on_click(cx.listener(move |store, _, _, cx| {
                    store.images.remove(index);
                    cx.notify();
                }))
                .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.stage)),
        )
        .into_any_element()
}

fn file_chip(
    cx: &mut Context<SessionStore>,
    index: usize,
    reference: &qaqh_client::ContentRef,
    t: &Palette,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded(px(RADIUS_CONTROL))
        .bg(t.surface_subtle)
        .border_1()
        .border_color(t.border_subtle)
        .child(Icon::new(IconName::FileText).with_size(Size::XSmall).text_color(t.dim))
        .child(
            div()
                .text_xs()
                .text_color(t.dim)
                .overflow_hidden()
                .whitespace_nowrap()
                .max_w(px(120.))
                .child(SharedString::from(
                    reference.0.as_str().chars().take(14).collect::<String>(),
                )),
        )
        .child(
            div()
                .id(SharedString::from(format!(
                    "composer-attach-remove-{index}"
                )))
                .cursor_pointer()
                .hover(|style| style.text_color(t.bad))
                .on_click(cx.listener(move |store, _, _, cx| {
                    store.attachments.remove(index);
                    cx.notify();
                }))
                .child(Icon::new(IconName::X).with_size(Size::XSmall).text_color(t.dim)),
        )
        .into_any_element()
}
