//! 封箱 / 开箱全套 UI 与原生 GPU 矢量动效（对齐 qaqh-electron ADR-016）：
//! - A 侧 `HandoffCard`：工作简述、上下文占用率、涉及文件数、阅读窗倒计时与「取消自动 / 立即继续 / 打开后继」、可折叠 Markdown 交接文档；
//! - B 侧 `HandoffOpenCard`：三源开箱判定（`HandoffSealed` / `handoff-doc:` 指纹 / 目录血缘 `handoff_from_session_id`）+「去源头」反向跳转 + 折叠/展开 + 原文切换；
//! - 三套 GPU 矢量动画（24×24 Lucide 描线坐标系 + `MotionLevel` 门控 + 基态=终态）：
//!   1. `box_seal_icon` / `box_open_icon`（卡头动态铰链盒子）；
//!   2. `seal_ritual_overlay`（2.45s 纸入盒→合盖→描勾→飞入卡头）与 `open_ritual_overlay`（1.80s 勾淡→掀盖→纸升起）；
//!   3. `fly_box_overlay`（680ms 抛物线跨区飞箱至侧栏目标会话行）。

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::assets::IconName;
use gpui_kit::base::SelectableText;
use gpui_kit::component::{Icon, Sizable, Size};
use gpui_kit::prelude::*;
use gpui_kit::{
    AnyElement, Bounds, Div, Entity, FontWeight, Hsla, LineCap, LineJoin, PathBuilder, PathStyle,
    Pixels, Point, SharedString, StrokeOptions, WeakEntity, Window, canvas, div, fill, point, px,
    relative, size,
};
use qaqh_gpui_kit::kit::{
    RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, relative_time, shadow_card, soft,
};
use qaqh_gpui_kit::motion::{accordion_reveal, bottom_fade_mask, effective_motion, spring_chevron};
use qaqh_gpui_kit::primitives::action_button;
use qaqh_gpui_kit::settings::{MotionLevel, Typography};
use qaqh_gpui_kit::theme::Palette;
use qaqh_core_vocab::ToolCallId;
use qaqh_ringing_v3::{BlockKind, BlockView};

use crate::store::{HandoffFacts, SessionStore, now_ms};

const SEAL_RITUAL_SECS: f32 = 2.45;
const OPEN_RITUAL_SECS: f32 = 1.80;
const ICON_SEAL_SECS: f32 = 2.40;
const ICON_OPEN_SECS: f32 = 2.20;
pub const FLY_BOX_SECS: f32 = 0.68;

/// B 侧开箱出处三件套（来源会话 + 工作简述 + 封箱时刻）。
#[derive(Clone, Debug)]
pub struct HandoffOpenSource {
    pub parent_session_id: qaqh_core_vocab::SessionId,
    pub work_summary: String,
    pub at_ms: u64,
}

#[derive(Default)]
struct AnchorMap {
    seal_icons: HashMap<String, Bounds<Pixels>>,
    sidebar_rows: HashMap<String, Bounds<Pixels>>,
    sidebar_list: Option<Bounds<Pixels>>,
    titlebar_toggle: Option<Bounds<Pixels>>,
}

/// 跨组件共享的窗口坐标锚点表：A 侧盒槽、侧栏各会话行、侧栏容器与 TitleBar 切换钮。
#[derive(Clone, Default)]
pub struct HandoffAnchors(Rc<RefCell<AnchorMap>>);

impl HandoffAnchors {
    pub fn record_seal_icon(&self, session_id: &str, bounds: Bounds<Pixels>) {
        self.0
            .borrow_mut()
            .seal_icons
            .insert(session_id.to_owned(), bounds);
    }

    pub fn seal_icon(&self, session_id: &str) -> Option<Bounds<Pixels>> {
        self.0.borrow().seal_icons.get(session_id).copied()
    }

    pub fn record_sidebar_row(&self, session_id: &str, bounds: Bounds<Pixels>) {
        self.0
            .borrow_mut()
            .sidebar_rows
            .insert(session_id.to_owned(), bounds);
    }

    pub fn record_sidebar_list(&self, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().sidebar_list = Some(bounds);
    }

    pub fn record_titlebar_toggle(&self, bounds: Bounds<Pixels>) {
        self.0.borrow_mut().titlebar_toggle = Some(bounds);
    }

    /// 飞箱落点：优先精准命中侧栏对应 `session_id` 行；次选侧栏列表区上方；再退回 TitleBar 侧栏钮。
    pub fn resolve_fly_target(&self, session_id: &str) -> Option<Point<Pixels>> {
        let map = self.0.borrow();
        if let Some(row) = map.sidebar_rows.get(session_id)
            && row.size.width > px(1.0)
            && row.size.height > px(1.0)
        {
            return Some(row.center());
        }
        if let Some(list) = map.sidebar_list
            && list.size.width > px(24.0)
            && list.size.height > px(16.0)
        {
            return Some(point(
                list.origin.x + list.size.width * 0.28,
                list.origin.y + (list.size.height - px(24.)).min(px(52.)).max(px(16.)),
            ));
        }
        map.titlebar_toggle.map(|bounds| bounds.center())
    }
}

/// 零布局侵入的坐标探测画布（挂在 `.relative()` 容器内即可在 prepaint 期捕获窗口矩形）。
pub fn anchor_probe(on_bounds: impl FnOnce(Bounds<Pixels>) + 'static) -> AnyElement {
    canvas(move |bounds, _, _| on_bounds(bounds), |_, _, _, _| {})
        .absolute()
        .inset_0()
        .into_any_element()
}

/// 解析投递指纹 `handoff-doc:<封箱会话>:<回合>` 中的来源会话 id。
pub fn handoff_doc_source(input_id: &str) -> Option<String> {
    let rest = input_id.strip_prefix("handoff-doc:")?;
    let parent = rest.split(':').next().unwrap_or("");
    (!parent.is_empty()).then(|| parent.to_owned())
}

/// B 侧开箱判定（仅作用于会话时间线中第一条 Input）。
pub fn detect_open_source(store: &SessionStore, entry_index: usize) -> Option<HandoffOpenSource> {
    let first_input = store
        .entries
        .iter()
        .position(|block| block.kind == BlockKind::User)?;
    if entry_index != first_input {
        return None;
    }
    let BlockView {
        kind: BlockKind::User,
        turn_id, content, ..
    } = store.entries.get(entry_index)?
    else {
        return None;
    };
    if crate::timeline::content_text(store, content).trim().is_empty() {
        return None;
    }

    if let Some(incoming) = &store.handoff_incoming
        && incoming.parent_session_id != store.session_id
    {
        return Some(incoming.clone());
    }

    let from_input_id = turn_id.as_ref().and_then(|tid| {
        store
            .turns
            .iter()
            .find(|turn| &turn.turn_id == tid)
            .and_then(|turn| turn.input_id.as_ref().map(|id| id.as_str()))
            .and_then(handoff_doc_source)
    });

    let parent = from_input_id
        .and_then(|raw| qaqh_core_vocab::SessionId::new(raw).ok())
        .or_else(|| store.handoff_from_session_id.clone())?;
    if parent == store.session_id {
        return None;
    }

    Some(HandoffOpenSource {
        parent_session_id: parent,
        work_summary: store
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty() && !title.starts_with("ses_"))
            .unwrap_or_default()
            .to_owned(),
        at_ms: store.session_created_at_ms,
    })
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn ease_out_standard(t: f32) -> f32 {
    // 近似 cubic-bezier(0.2, 0.0, 0.0, 1.0)
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(4)
}

fn segment_t(u: f32, start: f32, end: f32) -> f32 {
    if end <= start {
        return if u >= end { 1.0 } else { 0.0 };
    }
    ((u - start) / (end - start)).clamp(0.0, 1.0)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn rotate_about(px_x: f32, px_y: f32, ox: f32, oy: f32, deg: f32) -> (f32, f32) {
    if deg.abs() < 0.001 {
        return (px_x, px_y);
    }
    let rad = deg.to_radians();
    let (s, c) = rad.sin_cos();
    let dx = px_x - ox;
    let dy = px_y - oy;
    (ox + dx * c - dy * s, oy + dx * s + dy * c)
}

fn stroke_builder(width: f32) -> PathBuilder {
    PathBuilder::default().with_style(PathStyle::Stroke(StrokeOptions {
        line_width: width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    }))
}

fn paint_builder(builder: PathBuilder, color: Hsla, window: &mut Window) {
    if color.a <= 0.004 {
        return;
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// 在 24×24 Lucide 坐标系内绘制可动画的盒体、双铰链盖板、渐进对勾及上升箭头。
#[allow(clippy::too_many_arguments)]
fn paint_vector_box(
    window: &mut Window,
    center: Point<Pixels>,
    box_size: f32,
    color: Hsla,
    body_fill: Option<Hsla>,
    lid_open: f32,
    check_progress: f32,
    check_alpha: f32,
    spark: Option<f32>,
    rotation_deg: f32,
    stroke_px: f32,
) {
    let scale = box_size / 24.0;
    let map = |vx: f32, vy: f32| -> Point<Pixels> {
        let (rx, ry) = rotate_about(vx, vy, 12.0, 12.0, rotation_deg);
        point(
            center.x + px((rx - 12.0) * scale),
            center.y + px((ry - 12.0) * scale),
        )
    };

    // 盒身可选遮挡填充（封箱仪式中用于自然吞没落入盒口的纸张）。
    if let Some(fill_color) = body_fill
        && fill_color.a > 0.004
    {
        let mut fill_path = PathBuilder::fill();
        fill_path.add_polygon(
            &[map(4.0, 9.0), map(20.0, 9.0), map(20.0, 21.0), map(4.0, 21.0)],
            true,
        );
        paint_builder(fill_path, fill_color, window);
    }

    // 盒体矩形 (4,9) -> (20,21)
    let mut body = stroke_builder(stroke_px);
    body.add_polygon(
        &[map(4.0, 9.0), map(20.0, 9.0), map(20.0, 21.0), map(4.0, 21.0)],
        true,
    );
    paint_builder(body, color, window);

    // 左盖板 (4,9) -> (11,5)，绕铰链 (4,9) 外旋 -75° * lid_open
    let lid_open = lid_open.clamp(0.0, 1.0);
    let (lx, ly) = rotate_about(11.0, 5.0, 4.0, 9.0, -75.0 * lid_open);
    let mut lid_l = stroke_builder(stroke_px);
    lid_l.move_to(map(4.0, 9.0));
    lid_l.line_to(map(lx, ly));
    paint_builder(lid_l, color, window);

    // 右盖板 (20,9) -> (13,5)，绕铰链 (20,9) 外旋 +75° * lid_open
    let (rx, ry) = rotate_about(13.0, 5.0, 20.0, 9.0, 75.0 * lid_open);
    let mut lid_r = stroke_builder(stroke_px);
    lid_r.move_to(map(20.0, 9.0));
    lid_r.line_to(map(rx, ry));
    paint_builder(lid_r, color, window);

    // 对勾渐进描边 (8.5,15.5) -> (11,18) -> (15.5,13)
    let cp = check_progress.clamp(0.0, 1.0);
    let check_color = soft(color, color.a * check_alpha.clamp(0.0, 1.0));
    if cp > 0.01 && check_color.a > 0.004 {
        let d1 = 2.5f32.hypot(2.5);
        let d2 = 4.5f32.hypot(5.0);
        let total = d1 + d2;
        let drawn = cp * total;
        let mut check = stroke_builder(stroke_px);
        check.move_to(map(8.5, 15.5));
        if drawn <= d1 {
            let k = drawn / d1;
            check.line_to(map(8.5 + 2.5 * k, 15.5 + 2.5 * k));
        } else {
            check.line_to(map(11.0, 18.0));
            let k = ((drawn - d1) / d2).clamp(0.0, 1.0);
            check.line_to(map(11.0 + 4.5 * k, 18.0 - 5.0 * k));
        }
        paint_builder(check, check_color, window);
    }

    // 开箱上升箭头（bx-spark）：杆 (12,7)->(12,2) + 折角 (9,4.5)->(12,1.5)->(15,4.5)
    if let Some(sp) = spark {
        let sp = sp.clamp(0.0, 1.0);
        if sp > 0.01 {
            let dy = (1.0 - sp) * 5.0;
            let spark_color = soft(color, color.a * sp);
            let mut shaft = stroke_builder(stroke_px);
            shaft.move_to(map(12.0, 7.0 + dy));
            shaft.line_to(map(12.0, 2.0 + dy));
            paint_builder(shaft, spark_color, window);

            let mut arrow = stroke_builder(stroke_px);
            arrow.move_to(map(9.0, 4.5 + dy));
            arrow.line_to(map(12.0, 1.5 + dy));
            arrow.line_to(map(15.0, 4.5 + dy));
            paint_builder(arrow, spark_color, window);
        }
    }
}

/// 仪式内 46×54 交接纸张（卡片底 + 描边 + 四行文本纹路）。
fn paint_paper_sheet(
    window: &mut Window,
    center: Point<Pixels>,
    scale: f32,
    rotation_deg: f32,
    alpha: f32,
    t: &Palette,
) {
    let alpha = alpha.clamp(0.0, 1.0);
    if alpha <= 0.004 || scale <= 0.05 {
        return;
    }
    let hw = 23.0 * scale;
    let hh = 27.0 * scale;
    let map = |lx: f32, ly: f32| -> Point<Pixels> {
        let (rx, ry) = rotate_about(lx, ly, 0.0, 0.0, rotation_deg);
        point(center.x + px(rx), center.y + px(ry))
    };

    let corners = [
        map(-hw, -hh),
        map(hw, -hh),
        map(hw, hh),
        map(-hw, hh),
    ];
    let mut bg = PathBuilder::fill();
    bg.add_polygon(&corners, true);
    paint_builder(bg, soft(t.stage, alpha), window);

    let mut border = stroke_builder((1.2 * scale).max(0.8));
    border.add_polygon(&corners, true);
    paint_builder(border, soft(t.border, alpha), window);

    let line_ratios = [0.86_f32, 0.70, 0.82, 0.55];
    let inner_w = 32.0 * scale;
    let left_x = -inner_w * 0.5;
    let top_y = -hh + 11.0 * scale;
    let row_gap = 8.0 * scale;
    let line_color = soft(t.dim, 0.42 * alpha);
    for (i, ratio) in line_ratios.iter().enumerate() {
        let y = top_y + (i as f32) * row_gap;
        let mut line = stroke_builder((2.0 * scale).max(1.0));
        line.move_to(map(left_x, y));
        line.line_to(map(left_x + inner_w * ratio, y));
        paint_builder(line, line_color, window);
    }
}

/// 卡头封箱盒子图标（28×28 宿主槽位，内嵌 22px 描线图标，同时向 `HandoffAnchors` 上报坐标）。
fn box_seal_icon(
    color: Hsla,
    started_at: Option<Instant>,
    motion: MotionLevel,
    anchors: HandoffAnchors,
    session_id: String,
) -> Div {
    let icon_bg = soft(color, 0.10);
    div()
        .relative()
        .size(px(28.))
        .flex_shrink_0()
        .rounded(px(RADIUS_CONTROL))
        .bg(icon_bg)
        .border_1()
        .border_color(soft(color, 0.22))
        .child(
            canvas(
                move |bounds, _, _| {
                    anchors.record_seal_icon(&session_id, bounds);
                },
                move |bounds, _, window, _| {
                    let (lid_open, check_progress) = match (started_at, motion) {
                        (Some(t0), level) if level != MotionLevel::Off => {
                            let u = (t0.elapsed().as_secs_f32() / ICON_SEAL_SECS).clamp(0.0, 1.0);
                            if u < 1.0 {
                                window.request_animation_frame();
                            }
                            let close_t = ease_out_standard(segment_t(u, 0.12, 0.55));
                            let check_t = segment_t(u, 0.58, 0.88);
                            (1.0 - close_t, check_t)
                        }
                        _ => (0.0, 1.0),
                    };
                    paint_vector_box(
                        window,
                        bounds.center(),
                        18.0,
                        color,
                        None,
                        lid_open,
                        check_progress,
                        1.0,
                        None,
                        0.0,
                        1.7,
                    );
                },
            )
            .size_full(),
        )
}

/// 卡头开箱盒子图标（基态=终态：双盖外掀 75° + 上升箭头）。
fn box_open_icon(color: Hsla, started_at: Option<Instant>, motion: MotionLevel) -> Div {
    div()
        .relative()
        .size(px(28.))
        .flex_shrink_0()
        .rounded(px(RADIUS_CONTROL))
        .bg(soft(color, 0.10))
        .border_1()
        .border_color(soft(color, 0.22))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let (lid_open, spark) = match (started_at, motion) {
                        (Some(t0), level) if level != MotionLevel::Off => {
                            let u = (t0.elapsed().as_secs_f32() / ICON_OPEN_SECS).clamp(0.0, 1.0);
                            if u < 1.0 {
                                window.request_animation_frame();
                            }
                            let open_t = ease_out_standard(segment_t(u, 0.15, 0.62));
                            let spark_t = ease_out_cubic(segment_t(u, 0.45, 0.78));
                            (open_t, spark_t)
                        }
                        _ => (1.0, 1.0),
                    };
                    paint_vector_box(
                        window,
                        bounds.center(),
                        18.0,
                        color,
                        None,
                        lid_open,
                        0.0,
                        0.0,
                        Some(spark),
                        0.0,
                        1.7,
                    );
                },
            )
            .size_full(),
        )
}

/// A 侧大盒封箱仪式（2.45s）：交接纸显影丢入盒口 → 合盖 → 描勾 → 大盒沿弧线缩飞入卡头真槽位 → 淡出揭示。
fn seal_ritual_overlay(
    started_at: Instant,
    call_id: ToolCallId,
    entity: WeakEntity<SessionStore>,
    anchors: HandoffAnchors,
    session_id: String,
    t: Palette,
) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let u = (started_at.elapsed().as_secs_f32() / SEAL_RITUAL_SECS).clamp(0.0, 1.0);
            if u < 1.0 {
                window.request_animation_frame();
            } else {
                let entity = entity.clone();
                let cid = call_id.clone();
                cx.defer(move |cx| {
                    let _ = entity.update(cx, |store, cx| {
                        store.handoff_ritual_done.insert(cid.clone());
                        store.handoff_seal_rituals.remove(&cid);
                        cx.notify();
                    });
                });
            }

            let fade = if u <= 0.88 {
                1.0
            } else {
                1.0 - segment_t(u, 0.88, 1.0)
            };
            if fade <= 0.004 {
                return;
            }

            window.paint_quad(fill(bounds, soft(t.card, fade)));

            let stage_c = bounds.center();
            let box_rest = point(stage_c.x, stage_c.y + px(22.0));

            // 阶段一：纸片显影 → 悬停 → 旋转缩小落入盒口 (0..0.52)
            if u < 0.52 {
                let (paper_alpha, paper_dy, paper_scale, paper_rot) = if u < 0.08 {
                    let k = ease_out_standard(segment_t(u, 0.0, 0.08));
                    (k, lerp(-34.0, -18.0, k), lerp(0.9, 1.0, k), lerp(-6.0, -2.0, k))
                } else if u < 0.30 {
                    let k = ease_out_standard(segment_t(u, 0.08, 0.30));
                    (1.0, lerp(-18.0, -12.0, k), 1.0, lerp(-2.0, 0.0, k))
                } else {
                    let k = ease_out_standard(segment_t(u, 0.30, 0.52));
                    let a = if k > 0.65 {
                        1.0 - segment_t(k, 0.65, 1.0)
                    } else {
                        1.0
                    };
                    (a, lerp(-12.0, 24.0, k), lerp(1.0, 0.42, k), lerp(0.0, 7.0, k))
                };
                paint_paper_sheet(
                    window,
                    point(stage_c.x, stage_c.y + px(paper_dy)),
                    paper_scale,
                    paper_rot,
                    paper_alpha * fade,
                    &t,
                );
            }

            // 阶段二：大盒接纸、合盖、描勾、沿二次曲线缩飞入卡头真槽位
            let lid_open = 1.0 - ease_out_standard(segment_t(u, 0.52, 0.70));
            let check_progress = segment_t(u, 0.70, 0.84);
            let fly_t = ease_out_standard(segment_t(u, 0.68, 0.94));

            let target = anchors
                .seal_icon(&session_id)
                .map(|slot| slot.center())
                .unwrap_or_else(|| point(bounds.origin.x + px(30.0), bounds.origin.y + px(28.0)));
            let ctrl = point(
                (box_rest.x + target.x) * 0.5,
                (box_rest.y + target.y) * 0.5 - px(22.0),
            );
            let inv = 1.0 - fly_t;
            let pos = point(
                box_rest.x * (inv * inv) + ctrl.x * (2.0 * inv * fly_t) + target.x * (fly_t * fly_t),
                box_rest.y * (inv * inv) + ctrl.y * (2.0 * inv * fly_t) + target.y * (fly_t * fly_t),
            );
            let cur_size = lerp(64.0, 20.0, fly_t);
            let tilt = -6.0 * (4.0 * fly_t * (1.0 - fly_t));
            let fill_alpha = if u < 0.56 { fade } else { fade * (1.0 - fly_t) };

            paint_vector_box(
                window,
                pos,
                cur_size,
                soft(t.accent, fade),
                Some(soft(t.card, fill_alpha)),
                lid_open,
                check_progress,
                1.0,
                None,
                tilt,
                lerp(2.2, 1.6, fly_t),
            );
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// B 侧大盒开箱仪式（1.80s）：对勾淡出 → 掀盖 → 交接纸升起展开 → 遮罩淡出。
fn open_ritual_overlay(
    started_at: Instant,
    entity: WeakEntity<SessionStore>,
    t: Palette,
) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds, _, window, cx| {
            let u = (started_at.elapsed().as_secs_f32() / OPEN_RITUAL_SECS).clamp(0.0, 1.0);
            if u < 1.0 {
                window.request_animation_frame();
            } else {
                let entity = entity.clone();
                cx.defer(move |cx| {
                    let _ = entity.update(cx, |store, cx| {
                        store.handoff_open_ritual = None;
                        store.handoff_open_ritual_done = true;
                        cx.notify();
                    });
                });
            }

            let fade = if u <= 0.82 {
                1.0
            } else {
                1.0 - segment_t(u, 0.82, 1.0)
            };
            if fade <= 0.004 {
                return;
            }

            window.paint_quad(fill(bounds, soft(t.card, fade)));

            let stage_c = bounds.center();
            let box_c = point(stage_c.x, stage_c.y + px(18.0));
            let check_alpha = 1.0 - ease_out_standard(segment_t(u, 0.06, 0.24));
            let lid_open = ease_out_standard(segment_t(u, 0.28, 0.55));

            // 盒子先画在底层
            paint_vector_box(
                window,
                box_c,
                64.0,
                soft(t.accent, fade),
                None,
                lid_open,
                1.0,
                check_alpha,
                None,
                0.0,
                2.2,
            );

            // 交接纸后画，自盒口向前升起展开
            if u >= 0.46 {
                let (paper_alpha, paper_dy, paper_scale, paper_rot) = if u < 0.58 {
                    let k = ease_out_standard(segment_t(u, 0.46, 0.58));
                    (k, lerp(22.0, 4.0, k), lerp(0.42, 0.78, k), lerp(-3.0, 1.0, k))
                } else if u < 0.82 {
                    let k = ease_out_standard(segment_t(u, 0.58, 0.82));
                    (1.0, lerp(4.0, -24.0, k), lerp(0.78, 1.0, k), lerp(1.0, 2.0, k))
                } else {
                    let k = ease_out_standard(segment_t(u, 0.82, 1.0));
                    (1.0 - k, lerp(-24.0, -32.0, k), lerp(1.0, 1.03, k), lerp(2.0, 0.0, k))
                };
                paint_paper_sheet(
                    window,
                    point(stage_c.x, stage_c.y + px(paper_dy)),
                    paper_scale,
                    paper_rot,
                    paper_alpha * fade,
                    &t,
                );
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

/// 窗口级跨区飞箱浮层（680ms 抛物线飞行：A 卡头盒槽 → 侧栏后继会话行）。
pub fn fly_box_overlay(
    from: Point<Pixels>,
    target_session_id: String,
    started_at: Instant,
    anchors: HandoffAnchors,
    t: Palette,
) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |_, _, window, _| {
            let u = (started_at.elapsed().as_secs_f32() / FLY_BOX_SECS).clamp(0.0, 1.0);
            if u < 1.0 {
                window.request_animation_frame();
            }
            let to = anchors
                .resolve_fly_target(&target_session_id)
                .unwrap_or_else(|| point(px(88.0), px(96.0)));
            let ctrl = point((from.x + to.x) * 0.5, (from.y + to.y) * 0.5 - px(90.0));
            let k = ease_out_cubic(u);
            let inv = 1.0 - k;
            let pos = point(
                from.x * (inv * inv) + ctrl.x * (2.0 * inv * k) + to.x * (k * k),
                from.y * (inv * inv) + ctrl.y * (2.0 * inv * k) + to.y * (k * k),
            );
            let scale = if k < 0.5 {
                lerp(1.0, 0.92, k / 0.5)
            } else {
                lerp(0.92, 0.36, (k - 0.5) / 0.5)
            };
            let rot = if k < 0.5 {
                lerp(0.0, -10.0, k / 0.5)
            } else {
                lerp(-10.0, 6.0, (k - 0.5) / 0.5)
            };
            let alpha = if k < 0.5 {
                1.0
            } else {
                lerp(1.0, 0.35, (k - 0.5) / 0.5)
            };

            let halo_size = px(44.0 * scale);
            window.paint_quad(
                fill(
                    Bounds::centered_at(pos, size(halo_size, halo_size)),
                    soft(t.accent, 0.14 * alpha),
                )
                .corner_radii(gpui_kit::Corners::all(halo_size * 0.5)),
            );
            paint_vector_box(
                window,
                pos,
                40.0 * scale,
                soft(t.accent, alpha),
                Some(soft(t.card, 0.85 * alpha)),
                0.0,
                1.0,
                1.0,
                None,
                rot,
                (2.2 * scale).max(1.4),
            );
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

fn format_compact_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{}k", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

/// A 侧独立封箱交接卡（对应 Electron `HandoffCard`）。
pub fn render_seal_card(
    window: &mut Window,
    cx: &mut gpui_kit::App,
    entity: &Entity<SessionStore>,
    facts: &HandoffFacts,
    t: &Palette,
    ty: &Typography,
) -> Div {
    let motion = effective_motion(cx);
    let (
        session_id,
        anchors,
        has_seal,
        handoff_stay,
        handoff_eager,
        doc_open,
        ritual_started,
        md_extensions,
    ) = {
        let store = entity.read(cx);
        (
            store.session_id.as_str().to_owned(),
            store.handoff_anchors.clone(),
            store.handoff_seal.is_some(),
            store.handoff_stay,
            store.handoff_eager,
            store.handoff_doc_open.contains(&facts.call_id),
            store.handoff_seal_rituals.get(&facts.call_id).copied(),
            store.md_extensions.clone(),
        )
    };
    let icon_color = if facts.sealed { t.accent } else { t.warn };

    let occupancy = facts
        .context_used_tokens
        .zip(facts.context_window_tokens)
        .filter(|&(_, win)| win > 0)
        .map(|(used, win)| {
            let pct = ((used as f64 / win as f64) * 100.0).round() as u64;
            format!(
                "{pct}% · {} / {}",
                format_compact_tokens(used),
                format_compact_tokens(win)
            )
        });

    let header = div()
        .flex()
        .items_center()
        .gap_2p5()
        .child(box_seal_icon(
            icon_color,
            ritual_started,
            motion,
            anchors.clone(),
            session_id.clone(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
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
                                .child(if facts.sealed {
                                    "已封箱交接"
                                } else {
                                    "推断交接"
                                }),
                        )
                        .when(has_seal, |el| {
                            el.child(
                                div()
                                    .size_1p5()
                                    .rounded_full()
                                    .bg(t.ok),
                            )
                        }),
                )
                .child(div().text_xs().text_color(t.dim).child(if facts.sealed {
                    "当前会话上下文已封存并移交至后继会话"
                } else {
                    "机器面未完整读出，按交接文档正文推断"
                })),
        )
        .children(occupancy.map(|occ| {
            div()
                .flex_shrink_0()
                .px_2()
                .py_0p5()
                .rounded(px(RADIUS_CHIP))
                .bg(t.surface_subtle)
                .border_1()
                .border_color(t.border_subtle)
                .text_xs()
                .font_family(qaqh_gpui_kit::fonts::mono_font_family())
                .text_color(t.dim)
                .child(occ)
        }));

    let mut card = div()
        .relative()
        .overflow_hidden()
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(if facts.sealed {
            soft(t.accent, 0.28)
        } else {
            soft(t.warn, 0.30)
        })
        .bg(t.card)
        .shadow(shadow_card(t))
        .p_4()
        .flex()
        .flex_col()
        .gap_2p5()
        .child(header);

    if let Some(summary) = facts
        .summary
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        card = card.child(
            div()
                .text_sm()
                .text_color(t.text)
                .child(format!("工作简述：{summary}")),
        );
    }

    if !facts.modified_files.is_empty() || !facts.auto_continue {
        let mut meta = div().flex().flex_wrap().items_center().gap_3();
        if !facts.modified_files.is_empty() {
            meta = meta.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(t.dim)
                    .child(Icon::new(IconName::GitFork).with_size(Size::XSmall).text_color(t.dim))
                    .child(format!("涉及 {} 个文件", facts.modified_files.len())),
            );
        }
        if !facts.auto_continue {
            meta = meta.child(
                div()
                    .text_xs()
                    .text_color(t.dim)
                    .child("模型声明本次交接暂不自动跳转，需要时可手动打开后继"),
            );
        }
        card = card.child(meta);
    }

    if facts.sealed
        && !has_seal
        && !handoff_stay
        && let Some(planned_open) = facts.planned_open_at_ms
    {
        let total = planned_open
            .saturating_sub(facts.sealed_at_ms.unwrap_or(0))
            .max(1) as f32;
        let remaining = planned_open.saturating_sub(now_ms());
        let fraction = if remaining == 0 {
            0.0
        } else {
            ((remaining as f32 / total).clamp(0.0, 1.0) * 1000.0).round() / 1000.0
        };
        let label = if handoff_eager {
            SharedString::from("已确认立即继续，等待内核落盘后自动跳转…")
        } else if remaining == 0 {
            SharedString::from("阅读窗已结束，等待后继会话落盘…")
        } else {
            SharedString::from(format!("{}s 后自动继续交接", remaining.div_ceil(1000)))
        };
        let weak_cancel = entity.downgrade();
        card = card.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_2p5()
                .py_1p5()
                .rounded(px(RADIUS_CONTROL))
                .bg(t.surface_subtle)
                .child(
                    div()
                        .flex_1()
                        .h_1p5()
                        .rounded_full()
                        .bg(soft(t.faint, 0.16))
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .rounded_full()
                                .bg(t.accent)
                                .w(relative(fraction)),
                        ),
                )
                .child(div().text_xs().text_color(t.dim).child(label))
                .child(action_button(
                    cx,
                    format!("handoff-cancel-{}", facts.call_id.as_str()),
                    "取消自动",
                    t.dim,
                    move |_, _, cx: &mut gpui_kit::App| {
                        let _ = weak_cancel.update(cx, |store, cx| {
                            store.cancel_handoff_auto();
                            cx.notify();
                        });
                    },
                    t,
                )),
        );
    }

    let weak_primary = entity.downgrade();
    let primary_btn = if has_seal {
        action_button(
            cx,
            format!("handoff-open-child-{}", facts.call_id.as_str()),
            "打开后继会话 →",
            t.accent,
            move |_, _, cx: &mut gpui_kit::App| {
                let _ = weak_primary.update(cx, |store, cx| {
                    store.open_handoff_successor(cx);
                });
            },
            t,
        )
        .into_any_element()
    } else if facts.sealed {
        action_button(
            cx,
            format!("handoff-eager-{}", facts.call_id.as_str()),
            if handoff_eager {
                "正在前往后继…"
            } else {
                "立即继续"
            },
            t.accent,
            move |_, _, cx: &mut gpui_kit::App| {
                let _ = weak_primary.update(cx, |store, cx| {
                    store.arm_handoff_eager();
                    cx.notify();
                });
            },
            t,
        )
        .into_any_element()
    } else {
        let summary_fork = facts.summary.clone();
        let doc_fork = facts.doc.clone();
        action_button(
            cx,
            format!("handoff-fork-{}", facts.call_id.as_str()),
            "继续这条交接 →",
            t.accent,
            move |_, _, cx: &mut gpui_kit::App| {
                let _ = weak_primary.update(cx, |store, cx| {
                    store.fork_handoff(summary_fork.clone(), doc_fork.clone(), cx);
                });
            },
            t,
        )
        .into_any_element()
    };

    let weak_toggle = entity.downgrade();
    let toggle_cid = facts.call_id.clone();
    let doc_toggle = div()
        .id(SharedString::from(format!(
            "handoff-doc-toggle-{}",
            facts.call_id.as_str()
        )))
        .px_2p5()
        .py_1()
        .rounded(px(RADIUS_CHIP))
        .flex()
        .items_center()
        .gap_1()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(t.dim)
        .cursor_pointer()
        .hover(|s| s.bg(t.hover).text_color(t.text))
        .on_click(move |_, _, cx| {
            let _ = weak_toggle.update(cx, |store, cx| {
                if !store.handoff_doc_open.remove(&toggle_cid) {
                    store.handoff_doc_open.insert(toggle_cid.clone());
                }
                cx.notify();
            });
        })
        .child(if doc_open {
            "收起交接文档"
        } else {
            "查看交接文档"
        })
        .child(spring_chevron(
            SharedString::from(format!("handoff-doc-chev-{}", facts.call_id.as_str())),
            doc_open,
            t.dim,
            motion != MotionLevel::Off,
        ));

    card = card.child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(primary_btn)
            .child(doc_toggle),
    );

    let doc_text = facts.doc.clone();
    let doc_cid = facts.call_id.as_str().to_owned();
    let weak_doc = entity.downgrade();
    if let Some(panel) = accordion_reveal(
        SharedString::from(format!("handoff-doc-panel-{doc_cid}")),
        doc_open,
        window,
        cx,
        |_cx| {
            div()
                .id(SharedString::from(format!("handoff-doc-scroll-{doc_cid}")))
                .max_h(px(360.))
                .overflow_y_scroll()
                .rounded(px(RADIUS_CONTROL))
                .bg(t.surface_subtle)
                .border_1()
                .border_color(t.border_subtle)
                .p_3()
                .child(crate::markdown::render_document(
                    SharedString::from(format!("handoff-doc-md-{doc_cid}")),
                    &doc_text,
                    &md_extensions,
                    weak_doc,
                    t,
                    ty,
                ))
                .into_any_element()
        },
    ) {
        card = card.child(panel);
    }

    if motion == MotionLevel::Full
        && let Some(started) = ritual_started
    {
        card = card.child(seal_ritual_overlay(
            started,
            facts.call_id.clone(),
            entity.downgrade(),
            anchors,
            session_id,
            *t,
        ));
    }

    card
}

/// B 侧开箱交接卡（对应 Electron `HandoffOpenCard`）。
pub fn render_open_card(
    _window: &mut Window,
    cx: &mut gpui_kit::App,
    entity: &Entity<SessionStore>,
    doc: &str,
    source: &HandoffOpenSource,
    t: &Palette,
    ty: &Typography,
) -> Div {
    let motion = effective_motion(cx);
    let (open_ritual, open_raw, open_expanded, md_extensions) = {
        let store = entity.read(cx);
        (
            store.handoff_open_ritual,
            store.handoff_open_raw,
            store.handoff_open_expanded,
            store.md_extensions.clone(),
        )
    };
    let summary: String = source
        .work_summary
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let short_parent: String = source.parent_session_id.as_str().chars().take(12).collect();

    let mut meta = div().flex().flex_wrap().items_center().gap_2().text_xs().text_color(t.dim);
    if source.at_ms > 0 {
        meta = meta.child(format!("封箱于 {}", relative_time(source.at_ms / 1000)));
    }
    meta = meta.child(format!("来源会话 {short_parent}…"));

    let weak_src = entity.downgrade();
    let parent_id = source.parent_session_id.clone();
    meta = meta.child(
        div()
            .id("handoff-open-jump-source")
            .px_2()
            .py_0p5()
            .rounded(px(RADIUS_CHIP))
            .bg(soft(t.accent, 0.10))
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(t.accent)
            .cursor_pointer()
            .hover(|s| s.bg(soft(t.accent, 0.18)))
            .on_click(move |_, _, cx| {
                let _ = weak_src.update(cx, |store, cx| {
                    store.open_session(parent_id.clone(), false, cx);
                });
            })
            .flex()
            .items_center()
            .gap_1()
            .child(Icon::new(IconName::ArrowUpRight).with_size(Size::XSmall).text_color(t.accent))
            .child("去源头"),
    );

    let header = div()
        .px_4()
        .py_3()
        .border_b_1()
        .border_color(t.border_subtle)
        .flex()
        .items_start()
        .gap_2p5()
        .child(box_open_icon(t.accent, open_ritual, motion))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_0p5()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.text)
                                .child("开箱交接"),
                        )
                        .when(!summary.is_empty(), |el| {
                            el.child(div().text_xs().text_color(t.dim).child(summary))
                        }),
                )
                .child(meta),
        );

    let body_content = if open_raw {
        div()
            .rounded(px(RADIUS_CONTROL))
            .bg(t.surface_subtle)
            .p_3()
            .text_xs()
            .font_family(qaqh_gpui_kit::fonts::mono_font_family())
            .text_color(t.text)
            .child(
                SelectableText::new("handoff-open-raw-sel", doc.to_owned())
                    .selection_color(t.selection_wash),
            )
            .into_any_element()
    } else {
        crate::markdown::render_document(
            "handoff-open-doc-md",
            doc,
            &md_extensions,
            entity.downgrade(),
            t,
            ty,
        )
    };

    let body = if open_expanded {
        div().px_4().py_3().child(body_content)
    } else {
        div()
            .px_4()
            .py_3()
            .child(
                div()
                    .relative()
                    .max_h(px(168.))
                    .overflow_hidden()
                    .child(body_content)
                    .child(
                        bottom_fade_mask(t.card, 40.)
                            .absolute()
                            .bottom_0()
                            .left_0()
                            .right_0(),
                    ),
            )
    };

    let weak_exp = entity.downgrade();
    let weak_raw = entity.downgrade();
    let footer = div()
        .px_4()
        .py_2()
        .border_t_1()
        .border_color(t.border_subtle)
        .flex()
        .items_center()
        .gap_2()
        .child(action_button(
            cx,
            "handoff-open-expand".into(),
            if open_expanded {
                "收起文档"
            } else {
                "展开全文"
            },
            t.dim,
            move |_, _, cx: &mut gpui_kit::App| {
                let _ = weak_exp.update(cx, |store, cx| {
                    store.handoff_open_expanded = !store.handoff_open_expanded;
                    cx.notify();
                });
            },
            t,
        ))
        .child(action_button(
            cx,
            "handoff-open-raw-toggle".into(),
            if open_raw {
                "渲染视图"
            } else {
                "查看原文"
            },
            t.dim,
            move |_, _, cx: &mut gpui_kit::App| {
                let _ = weak_raw.update(cx, |store, cx| {
                    store.handoff_open_raw = !store.handoff_open_raw;
                    cx.notify();
                });
            },
            t,
        ));

    let mut card = div()
        .relative()
        .overflow_hidden()
        .rounded(px(RADIUS_CARD))
        .border_1()
        .border_color(soft(t.accent, 0.24))
        .bg(t.card)
        .shadow(shadow_card(t))
        .flex()
        .flex_col()
        .child(header)
        .child(body)
        .child(footer);

    if motion == MotionLevel::Full
        && let Some(started) = open_ritual
    {
        card = card.child(open_ritual_overlay(started, entity.downgrade(), *t));
    }

    card
}
