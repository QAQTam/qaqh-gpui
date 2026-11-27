//! GPU 物理动效引擎(DESIGN LAW v3.0 · v5.0.0):
//! 接入 gpui_kit::base::motion 的 Spring / Presence / MotionReveal / Stagger / Keyframes,
//! 全链受 MotionLevel(流畅物理 / 精简动效 / 零动效)与系统 reduce_motion() 门控,静止态零帧请求。

use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::base::{
    Easing, Keyframe, Keyframes, MotionReveal, Presence, PresenceSample, SignedDuration, Spring,
    Stagger, StaggerOrigin, Timing, Transition, TransitionId, animate_keyframes, spring,
    transition_with_status,
};
use gpui_kit::component::shimmer::ShimmerText;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, Div, ElementId, FontWeight, Hsla, Radians,
    SharedString, SpringAnimation, SpringConfig, Window, div, linear_color_stop, linear_gradient,
    px,
};

use crate::settings::MotionLevel;
use crate::theme::Palette;

/// 面板展开/收折物理弹簧(侧栏 / 右侧 Inspector)。
pub const SPRING_PANEL: SpringConfig = SpringConfig::new(260.0, 30.0, 1.0);

/// 轻量交互弹跳曲线。
pub const SPRING_SNAPPY: SpringConfig = SpringConfig::new(380.0, 34.0, 1.0);

/// 折叠箭头旋转弹簧(0° ↔ 90°)。
pub const SPRING_CHEVRON: SpringConfig = SpringConfig::new(400.0, 32.0, 1.0);

/// 抽屉/面板像素宽度弹簧策略(快速响应,几乎无过冲,半像素收敛)。
pub fn spring_panel_policy() -> Spring {
    Spring::new(Duration::from_millis(260))
        .with_damping(0.94)
        .with_epsilon(0.5)
}

pub struct SplitDrawerSample {
    pub full_width: f32,
    pub animated_width: f32,
    pub card_width: f32,
}

/// 采样左右二分侧边抽屉的物理宽度(与左侧工作区列表共用 `spring_panel_policy`)。
/// 每帧必须无条件调用以保持 `0.0` 初值锚定;完全收起后返回 `None`。
pub fn sample_split_drawer(
    id: impl Into<TransitionId>,
    open: bool,
    sibling_open: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<SplitDrawerSample> {
    let viewport_w = f32::from(window.viewport_size().width);
    let available = (viewport_w - 16.0).max(320.0);
    let fraction = if sibling_open { 0.36 } else { 0.52 };
    let full_width = (available * fraction)
        .round()
        .clamp(320.0, (available - 240.0).max(320.0));
    let target_width = if open { full_width } else { 0.0 };
    let animated_width = spring_f32(id, target_width, spring_panel_policy(), window, cx).max(0.0);
    if !open && animated_width <= 0.5 {
        return None;
    }
    Some(SplitDrawerSample {
        full_width,
        animated_width,
        card_width: (full_width - 8.0).max(280.0),
    })
}

/// 包装右侧二分抽屉卡片:外层裁切与淡入淡出,内层锁定目标宽免除文本重排并叠加 16px 平滑滑入。
pub fn wrap_split_drawer(sample: SplitDrawerSample, card: AnyElement, cx: &App) -> Div {
    let ratio = (sample.animated_width / sample.full_width).clamp(0.0, 1.0);
    let gutter = if effective_motion(cx) == MotionLevel::Full {
        8.0 + (1.0 - ratio) * 16.0
    } else {
        8.0
    };
    div()
        .h_full()
        .min_h_0()
        .w(px(sample.animated_width))
        .max_w(gpui_kit::relative(0.60))
        .opacity(ratio)
        .flex_shrink_0()
        .overflow_hidden()
        .child(
            div()
                .w(px(sample.full_width))
                .h_full()
                .min_h_0()
                .pl(px(gutter))
                .flex_shrink_0()
                .child(card),
        )
}

/// 手风琴/折叠内容归一化(0..1)弹簧策略。
pub fn spring_reveal_policy() -> Spring {
    Spring::new(Duration::from_millis(210))
        .with_damping(0.95)
        .with_epsilon(0.004)
}

/// 浮层与模态容器弹簧策略。
pub fn spring_modal_policy() -> Spring {
    Spring::new(Duration::from_millis(220))
        .with_damping(0.90)
        .with_epsilon(0.003)
}

/// 解析当前生效的动效档位:系统 reduce_motion 优先级最高。
pub fn effective_motion(cx: &App) -> MotionLevel {
    if cx.reduce_motion() {
        MotionLevel::Off
    } else {
        crate::settings::get(cx).appearance.motion_level
    }
}

/// 速度连续可中途折返的浮点弹簧插值:
/// - Full: gpui-base 物理积分(静止后零帧请求);
/// - Reduced: 110ms 轻柔 EaseOut 过渡;
/// - Off: 瞬切(同步停掉弹簧旅行态防切换回来时跳变)。
pub fn spring_f32(
    id: impl Into<TransitionId>,
    target: f32,
    policy: Spring,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    match effective_motion(cx) {
        MotionLevel::Full => spring(id, target, policy, window, cx),
        MotionLevel::Reduced => {
            let policy = Transition::new(Duration::from_millis(110)).easing(Easing::EaseOut);
            transition_with_status(id, target, policy, window, cx).value
        }
        MotionLevel::Off => spring(id, target, policy.with_travel(false), window, cx),
    }
}

/// 浮层/模态窗真实进退场采样(支持退出播完再卸载 & 半途反向打断)。
pub fn presence_overlay(
    id: impl Into<TransitionId>,
    open: bool,
    duration_ms: u64,
    window: &mut Window,
    cx: &mut App,
) -> PresenceSample {
    let transition = match effective_motion(cx) {
        MotionLevel::Off => Transition::new(Duration::ZERO),
        MotionLevel::Reduced => {
            Transition::new(Duration::from_millis((duration_ms / 2).max(60))).easing(Easing::EaseOut)
        }
        MotionLevel::Full => Transition::new(Duration::from_millis(duration_ms))
            .easing(Easing::CubicBezier {
                x1: 0.2,
                y1: 0.0,
                x2: 0.0,
                y2: 1.0,
            }),
    };
    Presence::new(id, open)
        .transition(transition)
        .sample(window, cx)
}

/// 实测高度裁切 + 淡入淡出平滑手风琴:
/// 收起稳定后返回 None(彻底卸载子树,零布局绘制成本)。
pub fn accordion_reveal(
    id: impl Into<ElementId> + Clone,
    open: bool,
    window: &mut Window,
    cx: &mut App,
    build: impl FnOnce(&mut App) -> AnyElement,
) -> Option<AnyElement> {
    let elem_id: ElementId = id.into();
    let progress = spring_f32(
        (elem_id.clone(), "reveal"),
        if open { 1.0 } else { 0.0 },
        spring_reveal_policy(),
        window,
        cx,
    )
    .clamp(0.0, 1.0);
    if !open && progress <= 0.004 {
        return None;
    }
    let content = build(cx);
    if progress >= 0.999 {
        return Some(
            div()
                .flex_shrink_0()
                .w_full()
                .child(content)
                .into_any_element(),
        );
    }
    let wrapped = div().w_full().opacity(progress).child(content).into_any_element();
    Some(
        div()
            .flex_shrink_0()
            .w_full()
            .child(MotionReveal::new(elem_id, progress, wrapped))
            .into_any_element(),
    )
}

/// 0° ↔ 90° 物理弹簧折叠指示箭头(ChevronRight)。
pub fn spring_chevron(
    id: impl Into<ElementId>,
    expanded: bool,
    color: Hsla,
    animate: bool,
) -> AnyElement {
    let target = if expanded {
        std::f32::consts::FRAC_PI_2
    } else {
        0.0
    };
    let icon = Icon::new(IconName::ChevronRight)
        .with_size(Size::XSmall)
        .text_color(color);
    if animate {
        icon.with_spring(
            id.into(),
            SpringAnimation::new(SPRING_CHEVRON).to(target),
            |icon, rad| icon.rotate(Radians(rad)),
        )
        .into_any_element()
    } else {
        icon.rotate(Radians(target)).into_any_element()
    }
}

/// 错峰级联滑入动效(32ms 间隔,用于列表/建议卡片首次显现)。
pub fn stagger_enter(
    id: impl Into<TransitionId>,
    index: usize,
    count: usize,
    child: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    if effective_motion(cx) != MotionLevel::Full {
        return child;
    }
    let delay = Stagger::new(Duration::from_millis(32), StaggerOrigin::First).delay(index, count);
    let keyframes = Keyframes::try_new([
        Keyframe::new(0.0, 0.0_f32),
        Keyframe::new(1.0, 1.0_f32),
    ])
    .expect("valid stagger keyframes");
    let timing = Timing::new(Duration::from_millis(200))
        .delay(SignedDuration::positive(delay))
        .ease(Easing::EaseOut);
    let p = animate_keyframes(id, &keyframes, timing, window, cx).value;
    if p >= 0.999 {
        child
    } else {
        div()
            .w_full()
            .mt(px(8.0 * (1.0 - p)))
            .opacity(p)
            .child(child)
            .into_any_element()
    }
}

fn enter_progress(
    id: impl Into<TransitionId>,
    window: &mut Window,
    cx: &mut App,
) -> Option<(f32, f32)> {
    let duration_ms = match effective_motion(cx) {
        MotionLevel::Off => return None,
        MotionLevel::Reduced => 90,
        MotionLevel::Full => 160,
    };
    let keyframes = Keyframes::try_new([
        Keyframe::new(0.0, 0.0_f32),
        Keyframe::new(1.0, 1.0_f32),
    ])
    .expect("valid page keyframes");
    let timing = Timing::new(Duration::from_millis(duration_ms)).ease(Easing::EaseOut);
    let p = animate_keyframes(id, &keyframes, timing, window, cx).value;
    if p >= 0.999 {
        return None;
    }
    let shift = if effective_motion(cx) == MotionLevel::Reduced {
        0.0
    } else {
        6.0 * (1.0 - p)
    };
    Some((p, shift))
}

/// 页面切换轻微滑入 + 淡入(160ms EaseOut,保持 flex-1 全尺寸占位)。
pub fn page_enter(
    id: impl Into<TransitionId>,
    child: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some((p, shift)) = enter_progress(id, window, cx) else {
        return child;
    };
    div()
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .mt(px(shift))
        .opacity(p)
        .child(child)
        .into_any_element()
}

/// 滚动内容轻微滑入 + 淡入(160ms EaseOut,不限制纵向高度)。
pub fn scroll_enter(
    id: impl Into<TransitionId>,
    constrain_width: bool,
    child: AnyElement,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some((p, shift)) = enter_progress(id, window, cx) else {
        return child;
    };
    let wrapper = if constrain_width {
        div().w_full().min_w_0()
    } else {
        div().min_w_full()
    };
    wrapper
        .mt(px(shift))
        .opacity(p)
        .child(child)
        .into_any_element()
}

/// 0..1 往返正弦呼吸曲线。
pub fn pulse_easing(t: f32) -> f32 {
    0.5 - 0.5 * (t * std::f32::consts::TAU).cos()
}

/// 状态光点:固定 12px 占位免布局抖动;active 时外圈柔和呼吸扩散,静止态纯圆点。
pub fn live_dot(id: impl Into<SharedString>, color: Hsla, active: bool) -> AnyElement {
    let dot = div().size(px(6.)).rounded_full().bg(color);
    let container = div()
        .relative()
        .flex_shrink_0()
        .size(px(12.))
        .flex()
        .items_center()
        .justify_center();
    if active {
        let halo_id = SharedString::from(format!("live-halo-{}", id.into()));
        container
            .child(
                div()
                    .absolute()
                    .size(px(12.))
                    .rounded_full()
                    .bg(color)
                    .with_animation(
                        halo_id,
                        Animation::new(Duration::from_millis(1400))
                            .repeat()
                            .with_easing(pulse_easing),
                        move |style, value| style.opacity(0.10 + 0.30 * value),
                    ),
            )
            .child(dot)
            .into_any_element()
    } else {
        container.child(dot).into_any_element()
    }
}

/// GPU 扫光态文案(思考中 / 流式参数生成 / 流式回复尾部)。
pub fn shimmer_label(
    id: impl Into<SharedString>,
    text: impl Into<SharedString>,
    t: &Palette,
) -> AnyElement {
    ShimmerText::new(text.into())
        .id(id.into())
        .duration(Duration::from_millis(1800))
        .highlight_color(t.text)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.dim)
        .into_any_element()
}

/// 顶部渐隐过渡层:覆盖于有限高度滚动视窗上缘,让旧轨迹柔和淡入舞台背景。
pub fn top_fade_mask(stage_color: Hsla, height_px: f32) -> Div {
    div()
        .h(px(height_px))
        .w_full()
        .bg(linear_gradient(
            0.,
            linear_color_stop(crate::kit::soft(stage_color, 0.0), 0.0),
            linear_color_stop(stage_color, 1.0),
        ))
}

/// 底部渐隐过渡层:覆盖于时间线视口底缘,让滚动文本柔和沉入悬浮 Composer 后方。
pub fn bottom_fade_mask(stage_color: Hsla, height_px: f32) -> Div {
    div()
        .h(px(height_px))
        .w_full()
        .bg(linear_gradient(
            180.,
            linear_color_stop(crate::kit::soft(stage_color, 0.0), 0.0),
            linear_color_stop(stage_color, 1.0),
        ))
}
