//! 悬浮居中圆角 Composer(DESIGN LAW v2.0):
//! - 居中圆角岛(18px 圆角 + 柔和浮影 + 运行态轻光晕);
//! - 无框自适应多行 Textarea;
//! - 模型 / 推理强度 / 工具模式全走可见 Popover 下拉列表(告别盲点循环),零学习成本。

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Anchor, ClickEvent, Div, FontWeight, Hsla, IntoElement, ParentElement, SharedString,
    Styled, WeakEntity, Window, div, px,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::input::Textarea;
use gpui_kit::component::popover::{Popover, PopoverState};
use gpui_kit::component::{Disableable, Icon, Sizable, Size};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{
    RADIUS_CHIP, RADIUS_COMPOSER, RADIUS_CONTROL, RADIUS_POPOVER, shadow_composer, shadow_popover,
    soft,
};
use qaqh_gpui_kit::primitives::icon_button;
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
            if groups.is_empty() {
                return card.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_xs()
                        .text_color(t.dim)
                        .child("暂无可用模型，请先在设置中配置服务商。"),
                );
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
            card = card.child(list);
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
                .px_2p5()
                .pt_2()
                .pb_1p5()
                .child(Textarea::new(&store.composer).appearance(false).bordered(false))
                .children(strip)
                .child(toolbar),
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
