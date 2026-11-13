//! 通用控件原语(自 ui.rs 切分,P4)。跨 crate 共享,一律 pub。
use gpui::prelude::*;
use gpui::{AnyElement, ClickEvent, Div, Hsla, SharedString, Window, div, px, transparent_black};
use crate::theme::Palette;
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::{Sizable, Size};

/// 定色状态点(终态绿/红维度专用)。
pub fn status_dot_colored(label: &str, color: Hsla, _t: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1()
        .child(div().size_1p5().rounded_full().bg(color))
        .child(div().text_xs().text_color(color).child(SharedString::from(label.to_owned())))
}


/// 设置分区卡:小节标题 + 卡片容器,行间 hairline 由 setting_row 自带。
pub fn section_card(title: &str, rows: Vec<Div>, t: &Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_1p5()
        .child(crate::kit::section_label(title, t).px_1())
        .child(
            crate::kit::card(t)
                .flex()
                .flex_col()
                .px_3()
                .py_1()
                .children(rows),
        )
}

pub fn section_header(title: &str, t: &Palette) -> Div {
    div()
        .pt_4()
        .pb_1()
        .child(crate::kit::section_label(title, t))
}

/// 设置行(zed settings_ui 行布局):左标题+描述,右控件,行间分隔线。
pub fn setting_row(label: &str, desc: &str, control: AnyElement, t: &Palette) -> Div {
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

pub fn segmented_button(
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
        .rounded(px(crate::kit::RADIUS_CONTROL))
        .text_xs()
        .cursor_pointer()
        .when(active, |style| {
            style.bg(t.accent_bg).text_color(t.accent)
        })
        .when(!active, |style| {
            style.text_color(t.dim).hover(|style| style.bg(t.hover).text_color(t.text))
        })
        .on_click(listener)
        .child(label.into())
}

pub fn step_button(
    id: &'static str,
    glyph: &'static str,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded(px(crate::kit::RADIUS_CONTROL))
        .text_xs()
        .cursor_pointer()
        .min_w(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(t.dim)
        .hover(|style| style.bg(t.hover).text_color(t.text))
        .on_click(listener)
        .child(glyph)
}

pub fn action_button(
    cx: &gpui::App,
    id: String,
    label: &'static str,
    color: Hsla,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    t: &Palette,
) -> impl IntoElement {
    // Ghost 分级:透明底 + 彩色文字,hover 浮现面层;焦点/按压态交给组件库。
    // 主操作/破坏性动作不使用本函数,走 kit::button(Primary/Danger)。
    Button::new(SharedString::from(id))
        .with_size(Size::XSmall)
        .custom(
            ButtonCustomVariant::new(cx)
                .color(transparent_black())
                .foreground(color)
                .hover(t.hover)
                .active(t.hover),
        )
        .label(label)
        .on_click(listener)
}

