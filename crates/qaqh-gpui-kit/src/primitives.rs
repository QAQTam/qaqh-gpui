//! 通用控件原语(DESIGN LAW v2.0):圆角设置卡/行、分段、导航胶囊、TitleBar 胶囊。
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, ClickEvent, Div, FontWeight, Hsla, MouseButton, SharedString, Window, div, px,
};
use gpui_kit::assets::IconNamed;
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::{Icon, Sizable, Size};

use crate::kit::{RADIUS_CHIP, RADIUS_CONTROL};
use crate::theme::Palette;

/// 定色状态点(终态绿/红维度专用)。
pub fn status_dot_colored(label: &str, color: Hsla, _t: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .child(div().size_1p5().rounded_full().bg(color))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(SharedString::from(label.to_owned())),
        )
}

/// 设置分区卡:小节标题 + 悬浮圆角卡片容器。
pub fn section_card(title: &str, rows: Vec<Div>, t: &Palette) -> Div {
    let last_ix = rows.len().saturating_sub(1);
    let rows = rows
        .into_iter()
        .enumerate()
        .map(|(ix, row)| if ix == last_ix { row.border_b_0() } else { row });
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(crate::kit::section_label(title, t).px_1())
        .child(
            crate::kit::card(t)
                .flex()
                .flex_col()
                .px_4()
                .py_1()
                .children(rows),
        )
}

pub fn section_header(title: &str, t: &Palette) -> Div {
    div()
        .pt_3()
        .pb_1p5()
        .child(crate::kit::section_label(title, t))
}

/// 设置行:左标题+描述,右控件,柔和底线分隔。
pub fn setting_row(label: &str, desc: &str, control: AnyElement, t: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .py_3()
        .border_b_1()
        .border_color(t.border_subtle)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .max_w(px(400.))
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text)
                        .child(SharedString::from(label.to_owned())),
                )
                .when(!desc.is_empty(), |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(t.dim)
                            .child(SharedString::from(desc.to_owned())),
                    )
                }),
        )
        .child(div().flex_shrink_0().child(control))
}

pub fn segmented_button(
    id: String,
    label: impl Into<SharedString>,
    active: bool,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id))
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .text_xs()
        .font_weight(if active {
            FontWeight::MEDIUM
        } else {
            FontWeight::NORMAL
        })
        .cursor_pointer()
        .when(active, |style| {
            style
                .bg(t.accent_soft)
                .text_color(t.accent)
                .border_1()
                .border_color(crate::kit::soft(t.accent, 0.25))
        })
        .when(!active, |style| {
            style
                .text_color(t.dim)
                .hover(|style| style.bg(t.hover).text_color(t.text))
        })
        .on_click(listener)
        .child(label.into())
}

pub fn step_button(
    id: &'static str,
    icon: impl IconNamed,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .h(px(24.))
        .min_w(px(28.))
        .px_2()
        .rounded(px(RADIUS_CHIP))
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center()
        .bg(t.btn)
        .text_color(t.text)
        .hover(|style| style.bg(t.btn_hover))
        .on_click(listener)
        .child(Icon::new(icon).with_size(Size::XSmall).text_color(t.text))
}

pub fn action_button(
    cx: &gpui_kit::App,
    id: String,
    label: impl Into<SharedString>,
    color: Hsla,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    t: &Palette,
) -> impl IntoElement {
    Button::new(SharedString::from(id))
        .with_size(Size::XSmall)
        .rounded(ButtonRounded::Size(px(RADIUS_CHIP)))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(gpui_kit::transparent_black())
                .foreground(color)
                .hover(t.hover)
                .active(t.hover),
        )
        .label(label.into())
        .on_click(listener)
}

/// 幽灵图标按钮:icon + tooltip。
pub fn icon_button(
    cx: &gpui_kit::App,
    id: impl Into<SharedString>,
    icon: impl IconNamed,
    color: Hsla,
    tooltip: impl Into<SharedString>,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
    t: &Palette,
) -> impl IntoElement {
    Button::new(id.into())
        .with_size(Size::Small)
        .rounded(ButtonRounded::Size(px(RADIUS_CHIP)))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(gpui_kit::transparent_black())
                .foreground(color)
                .hover(t.hover)
                .active(t.hover),
        )
        .icon(Icon::new(icon).text_color(color))
        .tooltip(tooltip)
        .on_click(listener)
}

/// 圆角导航胶囊(侧栏入口 & 设置左侧目录共用):中性克制选中态,无厚边重影。
pub fn nav_pill(
    id: impl Into<SharedString>,
    icon: impl IconNamed,
    label: impl Into<SharedString>,
    badge: Option<SharedString>,
    active: bool,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let fg = if active { t.text } else { t.dim };
    div()
        .id(id.into())
        .w_full()
        .px_2p5()
        .py_1p5()
        .rounded(px(RADIUS_CONTROL))
        .flex()
        .items_center()
        .gap_2()
        .text_xs()
        .font_weight(if active {
            FontWeight::SEMIBOLD
        } else {
            FontWeight::MEDIUM
        })
        .cursor_pointer()
        .when(active, |row| row.bg(t.selected).text_color(t.text))
        .when(!active, |row| {
            row.text_color(t.dim)
                .hover(|style| style.bg(t.hover).text_color(t.text))
        })
        .on_click(listener)
        .child(Icon::new(icon).with_size(Size::XSmall).text_color(fg))
        .child(div().flex_1().truncate().child(label.into()))
        .children(badge.map(|text| {
            div()
                .px_1p5()
                .py(px(1.))
                .rounded_full()
                .bg(if active { t.card } else { t.btn })
                .text_xs()
                .text_color(if active { t.text } else { t.dim })
                .child(text)
        }))
        .into_any_element()
}

/// 顶部融合 TitleBar 胶囊按钮:克制中性选中态,内置鼠标按下阻止冒泡,避免被 Windows 拖拽区吞掉点击。
pub fn titlebar_pill(
    id: impl Into<SharedString>,
    icon: impl IconNamed,
    label: Option<SharedString>,
    badge: Option<SharedString>,
    active: bool,
    tooltip: impl Into<SharedString>,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let tooltip: SharedString = tooltip.into();
    let fg = if active { t.text } else { t.dim };
    div()
        .id(id.into())
        .h(px(28.))
        .px_2()
        .rounded(px(RADIUS_CHIP))
        .flex()
        .items_center()
        .gap_1p5()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(active, |pill| pill.bg(t.selected).text_color(t.text))
        .when(!active, |pill| {
            pill.text_color(t.dim)
                .hover(|style| style.bg(t.hover).text_color(t.text))
        })
        .tooltip(move |window, cx| {
            gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(listener)
        .child(Icon::new(icon).with_size(Size::XSmall).text_color(fg))
        .children(label)
        .children(badge.map(|text| {
            div()
                .px_1p5()
                .rounded_full()
                .bg(if active { t.card } else { t.btn })
                .text_xs()
                .text_color(if active { t.text } else { t.dim })
                .child(text)
        }))
        .into_any_element()
}
