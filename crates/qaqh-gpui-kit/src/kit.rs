//! 视觉原语层(DESIGN LAW v2.0):七档圆角梯级 / 四级光学阴影 / 舞台岛 / 卡片 / 胶囊 / 分段控件。

use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, BoxShadow, ClickEvent, Div, FontWeight, Hsla, InteractiveElement, IntoElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};
use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonRounded, ButtonVariants};
use gpui_kit::component::{Sizable, Size};

use crate::theme::Palette;

/// 圆角体系(v2.0):温润连续圆角梯级。
pub const RADIUS_STAGE: f32 = 18.0;
pub const RADIUS_COMPOSER: f32 = 18.0;
pub const RADIUS_BUBBLE: f32 = 16.0;
pub const RADIUS_CARD: f32 = 14.0;
pub const RADIUS_POPOVER: f32 = 12.0;
pub const RADIUS_CONTROL: f32 = 10.0;
pub const RADIUS_CHIP: f32 = 8.0;

/// 同色相透明变体。
pub fn soft(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// L1 悬浮舞台岛投影(轻柔层次影 + 顶部镜面内高光线)。
pub fn shadow_stage(t: &Palette) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(4.), t.shadow_ink)
            .blur_radius(px(14.))
            .spread_radius(px(-4.)),
        BoxShadow::new(px(0.), px(1.), soft(t.shadow_ink, t.shadow_ink.a * 0.7))
            .blur_radius(px(3.)),
        BoxShadow::new(px(0.), px(1.), t.specular).inset(),
    ]
}

/// 悬浮 Composer 投影;active(运行中/聚焦态)时叠加柔和中性外环。
pub fn shadow_composer(t: &Palette, active: bool) -> Vec<BoxShadow> {
    let mut shadows = vec![
        BoxShadow::new(px(0.), px(6.), t.shadow_ink)
            .blur_radius(px(16.))
            .spread_radius(px(-6.)),
        BoxShadow::new(px(0.), px(1.), soft(t.shadow_ink, t.shadow_ink.a * 0.6))
            .blur_radius(px(4.)),
    ];
    if active {
        shadows.push(
            BoxShadow::new(px(0.), px(0.), soft(t.accent, 0.22))
                .blur_radius(px(0.))
                .spread_radius(px(1.5)),
        );
    }
    shadows
}

/// L2 卡片/选中胶囊微抬升投影。
pub fn shadow_card(t: &Palette) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(1.), soft(t.shadow_ink, t.shadow_ink.a * 0.75))
            .blur_radius(px(5.))
            .spread_radius(px(-2.)),
        BoxShadow::new(px(0.), px(1.), t.specular).inset(),
    ]
}

/// L3 浮层 Popover / 模态窗 / 右键菜单投影。
pub fn shadow_popover(t: &Palette) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(10.), t.shadow_ink)
            .blur_radius(px(22.))
            .spread_radius(px(-4.)),
        BoxShadow::new(px(0.), px(2.), soft(t.shadow_ink, t.shadow_ink.a * 0.8))
            .blur_radius(px(6.)),
    ]
}

/// L1 悬浮圆角舞台岛(主对话 / Inspector / 代码 / 终端岛共用)。
pub fn stage_surface(t: &Palette) -> Div {
    div()
        .rounded(px(RADIUS_STAGE))
        .bg(t.stage)
        .border_1()
        .border_color(t.border_subtle)
        .shadow(shadow_stage(t))
        .overflow_hidden()
}

/// L2 圆角卡片容器。
pub fn card(t: &Palette) -> Div {
    div()
        .rounded(px(RADIUS_CARD))
        .bg(t.card)
        .border_1()
        .border_color(t.border_subtle)
        .shadow(shadow_card(t))
}

/// 带舒适内边距的内容卡。
pub fn card_padded(t: &Palette) -> Div {
    card(t).p_4()
}

/// 小节标题:清晰可读的字重与次级色阶。
pub fn section_label(label: &str, t: &Palette) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.dim)
        .child(SharedString::from(label.to_owned()))
}

/// 状态 pill:彩点 + 文字 + 柔和全圆胶囊。
pub fn pill(label: &str, color: Hsla, _t: &Palette) -> Div {
    div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_2p5()
        .py_0p5()
        .rounded_full()
        .bg(soft(color, 0.12))
        .child(div().size_1p5().rounded_full().bg(color))
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(color)
                .child(SharedString::from(label.to_owned())),
        )
}

/// 幽灵 chip:透明底,hover 面层浮现;active 态软底 accent。
pub fn ghost_chip(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
    tone: Hsla,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> gpui_kit::Stateful<Div> {
    div()
        .id(id.into())
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(active, |chip| chip.bg(t.accent_soft).text_color(t.accent))
        .when(!active, |chip| {
            chip.text_color(tone).hover(|style| style.bg(t.hover))
        })
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

/// 统一圆角按钮入口(gpui-component Button 承担焦点/按压/禁用态)。
pub fn button(
    cx: &gpui_kit::App,
    kind: BtnKind,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    let button = Button::new(id.into())
        .with_size(Size::Small)
        .rounded(ButtonRounded::Size(px(RADIUS_CONTROL)))
        .label(label);
    let button = match kind {
        BtnKind::Primary => button.primary(),
        BtnKind::Danger => button.custom(
            ButtonCustomVariant::new(cx)
                .color(soft(t.bad, 0.08))
                .foreground(t.bad)
                .hover(soft(t.bad, 0.14))
                .active(soft(t.bad, 0.18)),
        ),
        BtnKind::Soft => button.custom(
            ButtonCustomVariant::new(cx)
                .color(t.btn)
                .foreground(t.text)
                .hover(t.btn_hover)
                .active(t.btn_hover),
        ),
        BtnKind::Ghost => button.custom(
            ButtonCustomVariant::new(cx)
                .color(gpui_kit::transparent_black())
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
        .py(px(2.))
        .rounded(px(RADIUS_CHIP))
        .bg(t.surface_subtle)
        .border_1()
        .border_color(t.border_subtle)
        .text_xs()
        .text_color(t.dim)
        .child(SharedString::from(key.to_owned()))
}

/// 空状态/欢迎 hero:居中圆角舞台卡。
pub fn hero(t: &Palette) -> Div {
    card(t)
        .w(px(520.))
        .p(px(32.))
        .flex()
        .flex_col()
        .items_center()
        .gap_4()
}

/// 相对时间(≤4 字符紧凑英文单位档)。输入为 Unix 秒。
pub fn relative_time(updated_secs: u64) -> SharedString {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(updated_secs);
    if delta < 60 {
        "<1M".into()
    } else if delta < 3600 {
        SharedString::from(format!("{}M", delta / 60))
    } else if delta < 86_400 {
        SharedString::from(format!("{}H", delta / 3600))
    } else if delta < 30 * 86_400 {
        SharedString::from(format!("{}D", delta / 86_400))
    } else if delta < 365 * 86_400 {
        SharedString::from(format!("{}MO", delta / (30 * 86_400)))
    } else {
        SharedString::from(format!("{}Y+", delta / (365 * 86_400)))
    }
}

/// 分段控件容器:柔和中性底槽 + 选中项白卡微抬起。
pub fn segmented_strip(items: Vec<AnyElement>, t: &Palette) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap(px(2.))
        .p(px(2.))
        .rounded(px(RADIUS_CONTROL))
        .bg(t.surface_subtle)
        .border_1()
        .border_color(t.border_subtle)
        .children(items)
}

/// 分段控件子项:active = card 面浮起 + 微影 + 中性正文色;inactive = 透明 + dim。
pub fn segmented_item(
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    active: bool,
    t: &Palette,
    listener: impl Fn(&ClickEvent, &mut Window, &mut gpui_kit::App) + 'static,
) -> AnyElement {
    div()
        .id(id.into())
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
        .when(active, |item| {
            item.bg(t.card)
                .text_color(t.text)
                .shadow(shadow_card(t))
        })
        .when(!active, |item| {
            item.text_color(t.dim)
                .hover(|style| style.bg(t.hover).text_color(t.text))
        })
        .on_click(listener)
        .child(label.into())
        .into_any_element()
}
