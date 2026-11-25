//! 非侵入式内联 Ask 提问浮岛(Antigravity 风格 + DESIGN LAW v3.0):
//! 挂在 pending interactions 槽位(Composer 正上方),不铺全屏压暗遮罩:
//! - 支持多题步进(`1/N` + ←/→ 切换),保持 Composer 与上下文可读;
//! - 题干与选项走行内 Markdown(代码背底 / 文件路径与链接图标 / 点击直开 code_panel);
//! - 顶部一键折叠为单行提示条,支持 `1-9` / `↑↓` / `←→` / `Enter` / `Esc` 键盘导航;
//! - 选「其它」或直接输入即自动互斥取消已选项,回车直跳下一题或提交。

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Disableable, Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ClickEvent, Context, Div, Entity, Focusable, FontWeight, Hsla, KeyDownEvent,
    SharedString, Window, div, px, relative,
};
use qaqh_gpui_kit::kit::{
    RADIUS_CHIP, RADIUS_COMPOSER, RADIUS_CONTROL, shadow_card, soft,
};
use qaqh_gpui_kit::motion::accordion_reveal;
use qaqh_gpui_kit::theme::Palette;
use qaqh_gpui_wire::ask;
use qaqh_ringing_contract::fact::InteractionId;
use qaqh_ringing_contract::state::InteractionView;

use crate::store::SessionStore;
use crate::timeline::content_text;

fn keycap(label: impl Into<SharedString>, selected: bool, t: &Palette) -> Div {
    div()
        .w(px(20.))
        .h(px(20.))
        .flex_shrink_0()
        .rounded(px(RADIUS_CHIP))
        .flex()
        .items_center()
        .justify_center()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .bg(if selected { t.accent } else { t.surface_subtle })
        .text_color(if selected { t.on_accent } else { t.dim })
        .child(label.into())
}

fn header_icon_btn(
    cx: &gpui_kit::App,
    id: impl Into<SharedString>,
    icon: IconName,
    color: Hsla,
    tooltip: &'static str,
    disabled: bool,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let fg = if disabled { t.faint } else { color };
    let button = Button::new(id.into())
        .with_size(Size::XSmall)
        .rounded(ButtonRounded::Size(px(RADIUS_CHIP)))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(gpui_kit::transparent_black())
                .foreground(fg)
                .hover(t.hover)
                .active(t.selected),
        )
        .icon(Icon::new(icon).with_size(Size::XSmall).text_color(fg))
        .tooltip(tooltip)
        .on_click(listener);
    let button = if disabled { button.disabled(true) } else { button };
    button.into_any_element()
}

fn primary_pill(
    id: SharedString,
    label: &'static str,
    ready: bool,
    t: &Palette,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let bg = if ready {
        t.accent
    } else {
        soft(t.accent, 0.28)
    };
    let hover_bg = if ready {
        t.accent_hover
    } else {
        soft(t.accent, 0.36)
    };
    div()
        .id(id)
        .h(px(28.))
        .px_3()
        .rounded_full()
        .flex()
        .items_center()
        .gap_1p5()
        .bg(bg)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.on_accent)
        .cursor_pointer()
        .hover(move |style| style.bg(hover_bg))
        .on_click(on_click)
        .child(label)
        .child(
            Icon::new(IconName::CornerDownLeft)
                .with_size(Size::XSmall)
                .text_color(t.on_accent),
        )
        .into_any_element()
}

fn action_footer(
    iid: &InteractionId,
    allow_custom: bool,
    custom_field: &Entity<InputState>,
    option_count: usize,
    custom_active: bool,
    current_answered: bool,
    primary_label: &'static str,
    t: &Palette,
    on_focus_custom: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    on_skip: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    on_primary: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> Div {
    let mut footer = div().flex().items_center().gap_2().pt_0p5();
    if allow_custom {
        let inactive_bg = if option_count == 0 {
            t.surface_subtle
        } else {
            gpui_kit::transparent_black()
        };
        let inactive_border = if option_count == 0 {
            t.border_subtle
        } else {
            gpui_kit::transparent_black()
        };
        let mut custom_row = div()
            .id(SharedString::from(format!("ask-custom-{iid}")))
            .flex_1()
            .min_w_0()
            .px_2()
            .py_1()
            .rounded(px(RADIUS_CONTROL))
            .border_1()
            .border_color(if custom_active {
                soft(t.accent, 0.35)
            } else {
                inactive_border
            })
            .bg(if custom_active {
                t.accent_soft
            } else {
                inactive_bg
            })
            .hover(|style| style.bg(t.hover))
            .flex()
            .items_center()
            .gap_2()
            .on_click(on_focus_custom);
        if option_count > 0 {
            custom_row = custom_row.child(keycap(
                format!("{}", option_count + 1),
                custom_active,
                t,
            ));
        }
        custom_row = custom_row.child(
            div().flex_1().min_w_0().child(
                Input::new(custom_field)
                    .appearance(false)
                    .bordered(false)
                    .focus_bordered(false)
                    .small()
                    .w_full(),
            ),
        );
        footer = footer.child(custom_row);
    } else {
        footer = footer.child(div().flex_1());
    }

    footer
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    div()
                        .id(SharedString::from(format!("ask-skip-{iid}")))
                        .h(px(28.))
                        .px_2p5()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .text_xs()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.dim)
                        .cursor_pointer()
                        .hover(|style| style.bg(t.hover).text_color(t.text))
                        .on_click(on_skip)
                        .child("跳过"),
                )
                .child(primary_pill(
                    SharedString::from(format!("ask-next-{iid}")),
                    primary_label,
                    current_answered,
                    t,
                    on_primary,
                )),
        )
}

fn card_shell(id: SharedString, t: &Palette) -> gpui_kit::Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .rounded(px(RADIUS_COMPOSER))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .shadow(shadow_card(t))
        .px_3p5()
        .py_2p5()
        .flex()
        .flex_col()
        .gap_2()
}

pub fn render(
    store: &mut SessionStore,
    interaction: &InteractionView,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
    t: &Palette,
) -> AnyElement {
    let iid = interaction.interaction_id.clone();
    match ask::parse_request(&interaction.request) {
        Ok(questions) if !questions.is_empty() => {
            render_structured(store, &iid, &questions, window, cx, t)
        }
        _ => render_fallback(store, &iid, content_text(&interaction.request), window, cx, t),
    }
}

fn render_structured(
    store: &mut SessionStore,
    iid: &InteractionId,
    questions: &[ask::AskQuestion],
    window: &mut Window,
    cx: &mut Context<SessionStore>,
    t: &Palette,
) -> AnyElement {
    let ids: Vec<String> = questions.iter().map(|q| q.id.clone()).collect();
    let total = questions.len();
    let (active_index, collapsed, focus_handle, custom_field, chosen) = {
        let draft = store.ask_draft(iid, &ids, window, cx);
        let idx = draft.active_index.min(total - 1);
        draft.active_index = idx;
        (
            idx,
            draft.collapsed,
            draft.focus_handle.clone(),
            draft.customs[idx].clone(),
            draft.answers[idx].clone(),
        )
    };

    let question = &questions[active_index];
    let custom_value = custom_field.read(cx).value().trim().to_string();
    let custom_non_empty = !custom_value.is_empty();
    let custom_focused = custom_field.read(cx).focus_handle(cx).is_focused(window);
    let custom_active = custom_non_empty || custom_focused;
    let current_answered = custom_non_empty || chosen.is_some();
    let answered_count = (0..total)
        .filter(|&idx| store.ask_question_answered(iid, idx, cx))
        .count();
    let hint = store.ask_hint.clone();
    let entity = cx.entity();

    let mut header_left = div()
        .id(SharedString::from(format!("ask-head-{iid}")))
        .flex_1()
        .min_w_0()
        .flex()
        .items_start()
        .gap_2()
        .when(collapsed, |row| {
            let iid_expand = iid.clone();
            row.cursor_pointer().on_click(cx.listener(move |store, _, _, cx| {
                store.ask_toggle_collapsed(&iid_expand, cx);
            }))
        })
        .child(
            div().mt(px(2.)).flex_shrink_0().child(
                Icon::new(IconName::CircleQuestionMark)
                    .with_size(Size::Small)
                    .text_color(t.dim),
            ),
        );

    if collapsed {
        let mut collapsed_meta = div().flex_1().min_w_0().flex().items_center().gap_2();
        collapsed_meta = collapsed_meta.child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text)
                .truncate()
                .child(SharedString::from(question.question.clone())),
        );
        if total > 1 {
            collapsed_meta = collapsed_meta.child(
                div()
                    .flex_shrink_0()
                    .text_xs()
                    .text_color(t.dim)
                    .child(SharedString::from(format!("已答 {answered_count}/{total}"))),
            );
        }
        header_left = header_left.child(collapsed_meta);
    } else {
        let md_extensions = store.md_extensions.clone();
        let md_id = format!("ask-q-{}-{iid}-{active_index}", store.session_id);
        header_left = header_left.child(
            div()
                .flex_1()
                .min_w_0()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.text)
                .line_height(relative(1.45))
                .child(crate::markdown::render_inline(
                    md_id,
                    &question.question,
                    &md_extensions,
                    entity.downgrade(),
                    t,
                )),
        );
    }

    let mut header_right = div().flex_shrink_0().flex().items_center().gap_1();
    if total > 1 {
        let iid_prev = iid.clone();
        let iid_next = iid.clone();
        let can_prev = active_index > 0;
        let can_next = active_index + 1 < total;
        header_right = header_right
            .child(header_icon_btn(
                cx,
                format!("ask-step-prev-{iid}"),
                IconName::ArrowLeft,
                t.dim,
                "上一题 (←)",
                !can_prev,
                t,
                cx.listener(move |store, _, window, cx| {
                    if can_prev {
                        store.ask_set_step(&iid_prev, active_index - 1, cx);
                        focus_handle_focus(store, &iid_prev, window, cx);
                    }
                }),
            ))
            .child(
                div()
                    .px_1()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.dim)
                    .child(SharedString::from(format!("{}/{total}", active_index + 1))),
            )
            .child(header_icon_btn(
                cx,
                format!("ask-step-next-{iid}"),
                IconName::ArrowRight,
                t.dim,
                "下一题 (→)",
                !can_next,
                t,
                cx.listener(move |store, _, window, cx| {
                    if can_next {
                        store.ask_set_step(&iid_next, active_index + 1, cx);
                        focus_handle_focus(store, &iid_next, window, cx);
                    }
                }),
            ));
    }

    let iid_toggle = iid.clone();
    let iid_close = iid.clone();
    header_right = header_right
        .child(header_icon_btn(
            cx,
            format!("ask-collapse-{iid}"),
            if collapsed {
                IconName::ChevronUp
            } else {
                IconName::ChevronDown
            },
            t.dim,
            if collapsed { "展开提问" } else { "折叠提问 (Esc)" },
            false,
            t,
            cx.listener(move |store, _, _, cx| {
                store.ask_toggle_collapsed(&iid_toggle, cx);
            }),
        ))
        .child(header_icon_btn(
            cx,
            format!("ask-close-{iid}"),
            IconName::X,
            t.dim,
            "跳过此提问",
            false,
            t,
            cx.listener(move |store, _, _, cx| {
                store.ask_dismiss(&iid_close);
                cx.notify();
            }),
        ));

    let header = div()
        .flex()
        .items_start()
        .justify_between()
        .gap_3()
        .child(header_left)
        .child(header_right);

    let mut body_col = div().flex().flex_col().gap_1p5();
    if !question.options.is_empty() {
        let mut opts_scroll = div()
            .id(SharedString::from(format!("ask-opts-{iid}-{active_index}")))
            .max_h(px(260.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap_1();

        for (opt_idx, option) in question.options.iter().enumerate() {
            let is_selected =
                chosen.as_deref() == Some(option.as_str()) && !custom_non_empty;
            let opt_click = option.clone();
            let iid_opt = iid.clone();
            let mut row = div()
                .id(SharedString::from(format!(
                    "ask-opt-{iid}-{active_index}-{opt_idx}"
                )))
                .px_2()
                .py_1p5()
                .rounded(px(RADIUS_CONTROL))
                .border_1()
                .border_color(if is_selected {
                    soft(t.accent, 0.35)
                } else {
                    gpui_kit::transparent_black()
                })
                .bg(if is_selected {
                    t.accent_soft
                } else {
                    gpui_kit::transparent_black()
                })
                .when(!is_selected, |el| el.hover(|style| style.bg(t.hover)))
                .cursor_pointer()
                .flex()
                .items_start()
                .gap_2p5()
                .on_click(cx.listener(move |store, _, window, cx| {
                    store.ask_select_option(
                        &iid_opt,
                        active_index,
                        opt_click.clone(),
                        window,
                        cx,
                    );
                }))
                .child(div().mt(px(1.)).child(keycap(
                    format!("{}", opt_idx + 1),
                    is_selected,
                    t,
                )))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(t.text)
                        .line_height(relative(1.45))
                        .child(crate::markdown::render_inline_plain(
                            format!("ask-opt-md-{iid}-{active_index}-{opt_idx}"),
                            option,
                            t,
                        )),
                );
            if is_selected {
                row = row.child(
                    div().mt(px(3.)).flex_shrink_0().child(
                        Icon::new(IconName::Check)
                            .with_size(Size::XSmall)
                            .text_color(t.accent),
                    ),
                );
            }
            opts_scroll = opts_scroll.child(row);
        }
        body_col = body_col.child(opts_scroll);
    }

    if let Some(hint_text) = hint {
        body_col = body_col.child(
            div()
                .px_2()
                .text_xs()
                .text_color(t.bad)
                .child(SharedString::from(hint_text)),
        );
    }

    let iid_custom = iid.clone();
    let iid_skip = iid.clone();
    let iid_primary = iid.clone();
    let primary_label = if active_index + 1 < total {
        "继续"
    } else {
        "提交"
    };
    body_col = body_col.child(action_footer(
        iid,
        question.allow_custom,
        &custom_field,
        question.options.len(),
        custom_active,
        current_answered,
        primary_label,
        t,
        cx.listener(move |store, _, window, cx| {
            store.ask_focus_custom(&iid_custom, active_index, window, cx);
        }),
        cx.listener(move |store, _, _, cx| {
            store.ask_dismiss(&iid_skip);
            cx.notify();
        }),
        cx.listener(move |store, _, _, cx| {
            store.ask_advance_or_submit(&iid_primary, cx);
        }),
    ));

    let body_el = body_col.into_any_element();
    let iid_keys = iid.clone();
    let options_for_keys = question.options.clone();
    let allow_custom_keys = question.allow_custom;
    let card_focus = focus_handle.clone();

    let shell = card_shell(SharedString::from(format!("ask-dock-{iid}")), t)
        .track_focus(&focus_handle)
        .on_key_down(cx.listener(move |store, event: &KeyDownEvent, window, cx| {
            let m = &event.keystroke.modifiers;
            if m.control || m.alt || m.platform || m.function {
                return;
            }
            let key = event.keystroke.key.as_str();
            if key == "escape" && !collapsed {
                store.ask_toggle_collapsed(&iid_keys, cx);
                cx.stop_propagation();
                return;
            }
            if !card_focus.is_focused(window) {
                return;
            }
            match key {
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" => {
                    let digit = key.as_bytes()[0].saturating_sub(b'1') as usize;
                    if let Some(opt) = options_for_keys.get(digit) {
                        store.ask_select_option(
                            &iid_keys,
                            active_index,
                            opt.clone(),
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    } else if digit == options_for_keys.len() && allow_custom_keys {
                        store.ask_focus_custom(&iid_keys, active_index, window, cx);
                        cx.stop_propagation();
                    }
                }
                "up" | "down" if !options_for_keys.is_empty() => {
                    let len = options_for_keys.len();
                    let current = chosen
                        .as_deref()
                        .and_then(|val| options_for_keys.iter().position(|opt| opt == val));
                    let next = match (current, key) {
                        (None, "down") => 0,
                        (None, _) => len - 1,
                        (Some(pos), "down") => (pos + 1) % len,
                        (Some(pos), _) => (pos + len - 1) % len,
                    };
                    if let Some(opt) = options_for_keys.get(next) {
                        store.ask_select_option(
                            &iid_keys,
                            active_index,
                            opt.clone(),
                            window,
                            cx,
                        );
                        cx.stop_propagation();
                    }
                }
                "left" if total > 1 && active_index > 0 => {
                    store.ask_set_step(&iid_keys, active_index - 1, cx);
                    cx.stop_propagation();
                }
                "right" if total > 1 && active_index + 1 < total => {
                    store.ask_set_step(&iid_keys, active_index + 1, cx);
                    cx.stop_propagation();
                }
                "enter" => {
                    store.ask_advance_or_submit(&iid_keys, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
        }))
        .child(header);

    let revealed = accordion_reveal(
        SharedString::from(format!("ask-body-{iid}")),
        !collapsed,
        window,
        cx,
        |_| body_el,
    );
    shell.children(revealed).into_any_element()
}

fn render_fallback(
    store: &mut SessionStore,
    iid: &InteractionId,
    raw_text: &str,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
    t: &Palette,
) -> AnyElement {
    let ids = vec!["q1".to_string()];
    let (collapsed, custom_field) = {
        let draft = store.ask_draft(iid, &ids, window, cx);
        (draft.collapsed, draft.customs[0].clone())
    };
    let custom_value = custom_field.read(cx).value().trim().to_string();
    let custom_non_empty = !custom_value.is_empty();
    let custom_focused = custom_field.read(cx).focus_handle(cx).is_focused(window);
    let hint = store.ask_hint.clone();

    let iid_toggle = iid.clone();
    let iid_close = iid.clone();
    let header = div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .id(SharedString::from(format!("ask-head-raw-{iid}")))
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .when(collapsed, |row| {
                    let iid_expand = iid.clone();
                    row.cursor_pointer().on_click(cx.listener(move |store, _, _, cx| {
                        store.ask_toggle_collapsed(&iid_expand, cx);
                    }))
                })
                .child(
                    Icon::new(IconName::CircleQuestionMark)
                        .with_size(Size::Small)
                        .text_color(t.dim),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text)
                        .truncate()
                        .child("会话提问（原始格式）"),
                ),
        )
        .child(
            div()
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap_1()
                .child(header_icon_btn(
                    cx,
                    format!("ask-collapse-{iid}"),
                    if collapsed {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    },
                    t.dim,
                    if collapsed { "展开提问" } else { "折叠提问" },
                    false,
                    t,
                    cx.listener(move |store, _, _, cx| {
                        store.ask_toggle_collapsed(&iid_toggle, cx);
                    }),
                ))
                .child(header_icon_btn(
                    cx,
                    format!("ask-close-{iid}"),
                    IconName::X,
                    t.dim,
                    "跳过此提问",
                    false,
                    t,
                    cx.listener(move |store, _, _, cx| {
                        store.ask_dismiss(&iid_close);
                        cx.notify();
                    }),
                )),
        );

    let mut body_col = div().flex().flex_col().gap_1p5().child(
        div()
            .id(SharedString::from(format!("ask-raw-{iid}")))
            .max_h(px(180.))
            .overflow_y_scroll()
            .p_2p5()
            .rounded(px(RADIUS_CONTROL))
            .bg(t.surface_subtle)
            .text_xs()
            .text_color(t.dim)
            .child(SharedString::from(raw_text.to_owned())),
    );
    if let Some(hint_text) = hint {
        body_col = body_col.child(
            div()
                .px_2()
                .text_xs()
                .text_color(t.bad)
                .child(SharedString::from(hint_text)),
        );
    }

    let iid_custom = iid.clone();
    let iid_skip = iid.clone();
    let iid_primary = iid.clone();
    body_col = body_col.child(action_footer(
        iid,
        true,
        &custom_field,
        0,
        custom_non_empty || custom_focused,
        custom_non_empty,
        "提交",
        t,
        cx.listener(move |store, _, window, cx| {
            store.ask_focus_custom(&iid_custom, 0, window, cx);
        }),
        cx.listener(move |store, _, _, cx| {
            store.ask_dismiss(&iid_skip);
            cx.notify();
        }),
        cx.listener(move |store, _, _, cx| {
            store.ask_advance_or_submit(&iid_primary, cx);
        }),
    ));

    let body_el = body_col.into_any_element();
    let shell = card_shell(SharedString::from(format!("ask-dock-{iid}")), t).child(header);
    let revealed = accordion_reveal(
        SharedString::from(format!("ask-body-{iid}")),
        !collapsed,
        window,
        cx,
        |_| body_el,
    );
    shell.children(revealed).into_any_element()
}

fn focus_handle_focus(
    store: &SessionStore,
    iid: &InteractionId,
    window: &mut Window,
    cx: &mut Context<SessionStore>,
) {
    if let Some((_, draft)) = store.ask_drafts.iter().find(|(id, _)| id == iid) {
        draft.focus_handle.focus(window, cx);
    }
}
