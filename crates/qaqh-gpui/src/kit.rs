//! 视觉原语层(设计系统 M1):圆角/卡片/按钮分级/状态 pill/分段控件/键帽。
//! 全部 UI 经此拼装,不再就地手搓 div 样式——半径、面层、悬停态在这里收口。
//! 语言参照 zed:面层对比撑层级,hairline 只做收口,accent 只给主操作。

use gpui::prelude::*;
use gpui::AnyElement;
use gpui::ClickEvent;
use gpui::Div;
use gpui::Hsla;
use gpui::InteractiveElement;
use gpui::IntoElement;
use gpui::SharedString;
use gpui::StatefulInteractiveElement;
use gpui::Styled;
use gpui::Window;
use gpui::div;
use gpui::px;
use gpui_component::button::{Button, ButtonCustomVariant, ButtonVariants};
use gpui_component::{Sizable, Size};

use crate::theme::Palette;

/// 半径 token:控件 < 卡片 < 气泡;pill 用全圆。
pub const RADIUS_CONTROL: f32 = 7.0;
pub const RADIUS_CARD: f32 = 10.0;
pub const RADIUS_BUBBLE: f32 = 12.0;

/// 同色相透明变体:选中软底/悬停软底/危险软底共用。
pub fn soft(color: Hsla, alpha: f32) -> Hsla {
    Hsla {
        a: alpha,
        ..color
    }
}

/// 卡片容器:中圆角 + card 面层 + hairline 收口。层级交给面层,不叠阴影。
pub fn card(t: &Palette) -> Div {
    div()
        .rounded(px(RADIUS_CARD))
        .bg(t.card)
        .border_1()
        .border_color(t.border)
}

/// 带 padding 的内容卡。
pub fn card_padded(t: &Palette) -> Div {
    card(t).p_3()
}

/// 小节标题:大写感由字重与色阶承担(dim,medium)。
pub fn section_label(label: &str, t: &Palette) -> Div {
    div()
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(t.dim)
        .child(SharedString::from(label.to_owned()))
}

/// 状态 pill:彩点 + 文字 + 软底全圆。连接态/流态/活动态共用。
pub fn pill(label: &str, color: Hsla, _t: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(soft(color, 0.12))
        .child(div().size_1p5().rounded_full().bg(color))
        .child(
            div()
                .text_xs()
                .text_color(color)
                .child(SharedString::from(label.to_owned())),
        )
}

/// 幽灵 chip:透明底,hover 面层浮现;active 态软底 accent。次级动作/头部胶囊统一走它。
pub fn ghost_chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
    tone: Hsla,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<Div> {
    div()
        .id(id.into())
        .px_2()
        .py_0p5()
        .rounded(px(RADIUS_CONTROL))
        .text_xs()
        .cursor_pointer()
        .when(active, |chip| chip.bg(t.accent_bg).text_color(t.accent))
        .when(!active, |chip| chip.text_color(tone).hover(|style| style.bg(t.hover)))
        .on_click(listener)
        .child(label.into())
}

/// 按钮四级:Primary 主操作 / Soft 次主 / Ghost 普通 / Danger 破坏性。
#[derive(Clone, Copy)]
pub enum BtnKind {
    Primary,
    Soft,
    Ghost,
    Danger,
}

/// 统一按钮入口(gpui-component Button 承担焦点/按压/禁用态)。
/// Primary/Danger 用库内建实底变体(色值经 theme::sync_component_theme 映射);
/// Custom 变体背景被库按 20% 淡化,只适合 Ghost/Soft 这类"软底"语义。
pub fn button(
    cx: &gpui::App,
    kind: BtnKind,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let button = Button::new(id.into()).with_size(Size::Small).label(label);
    let button = match kind {
        BtnKind::Primary => button.primary(),
        BtnKind::Danger => button.danger(),
        BtnKind::Soft => button.custom(
            ButtonCustomVariant::new(cx)
                .color(t.accent_bg)
                .foreground(t.accent)
                .hover(t.btn_hover)
                .active(t.btn_hover),
        ),
        BtnKind::Ghost => button.custom(
            ButtonCustomVariant::new(cx)
                .color(gpui::transparent_black())
                .foreground(t.dim)
                .hover(t.hover)
                .active(t.hover),
        ),
    };
    button.on_click(listener).into_any_element()
}

/// 键帽 pill:快捷键提示。
pub fn kbd(key: &str, t: &Palette) -> Div {
    div()
        .px_1p5()
        .py(px(1.))
        .rounded(px(5.0))
        .bg(t.btn)
        .border_1()
        .border_color(t.border)
        .text_xs()
        .text_color(t.dim)
        .child(SharedString::from(key.to_owned()))
}

/// 空状态/欢迎 hero:居中卡片,glyph + 标题 + 描述 + 内容由调用方追加。
pub fn hero(t: &Palette) -> Div {
    card_padded(t)
        .w(px(460.))
        .py(px(36.))
        .flex()
        .flex_col()
        .items_center()
        .gap_3()
}

/// 分段控件容器:整体一块圆角面层,选中项浮起为 card 面 + accent 文字。
/// 子项用 [`segmented_item`] 构造。
pub fn segmented_strip(items: Vec<AnyElement>, t: &Palette) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_0p5()
        .p(px(3.))
        .rounded(px(RADIUS_CONTROL + 2.))
        .bg(soft(t.faint, if t.bg.l > 0.5 { 0.14 } else { 0.10 }))
        .children(items)
}

/// 分段控件子项:active = card 面浮起 + accent 文字;inactive = 透明 + dim。
pub fn segmented_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CONTROL))
        .text_xs()
        .cursor_pointer()
        .when(active, |item| {
            item.bg(t.card).text_color(t.accent)
        })
        .when(!active, |item| {
            item.text_color(t.dim)
                .hover(|style| style.bg(t.card_hover).text_color(t.text))
        })
        .on_click(listener)
        .child(label.into())
        .into_any_element()
}
