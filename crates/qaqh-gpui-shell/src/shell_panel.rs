//! 用户 shell 终端坞(ADR-0028,electron ShellTerminal/shell-tabs 的 GPUI 同位)。
//!
//! 生命周期对齐 electron shell-tabs.ts(2026-11-18 裁定):标签与进程的生命周期
//! **归 store,不归 DOM**——只有显式关标签才 `shell.close`;隐藏坞/切会话不断
//! 流(ws 订阅由桥托管,断联不 kill,§D5),后台编译与前台编译同时活着。
//!
//! 渲染与输入内核:每个页签持有独立 [`TerminalView`] (`alacritty_terminal` 无头
//! VT/ConPTY 状态机 + 原生 GPUI 画布),支持动态列行 `ShellResize`、`CSI 6 n`
//! 应答自动回写、鼠标选区复制、滚轮回滚与 Windows 中文 IME 组合输入;数据面直走
//! `RingingV2` `ShellStream`,零转换层。

use gpui_kit::prelude::*;
use gpui_kit::{Context, Div, Entity, FontWeight, Hsla, SharedString, Window, div, px};
use gpui_kit::component::{Icon, Sizable, Size, WindowExt as _};
use gpui_kit::assets::IconName;
use qaqh_gpui_kit::kit::{
    BtnKind, RADIUS_CARD, RADIUS_CHIP, RADIUS_CONTROL, RADIUS_STAGE, button, shadow_stage, soft,
};
use qaqh_gpui_kit::motion::{live_dot, spring_f32, spring_panel_policy};
use qaqh_gpui_kit::settings;
use qaqh_gpui_kit::theme::{ActiveTheme, Palette, terminal_palette};
use qaqh_gpui_session::terminal_view::TerminalView;
use qaqh_gpui_wire::bridge::{BridgeRequest, ConnectionState, ShellInfo, ShellUiStatus};

use crate::daemon::DaemonStore;

/// `shell.open` 闭集里暴露给用户的三个档位(electron SHELL_PROFILES 同集;
/// argv 始终由 daemon 解析,非法名在注册表当场拒绝)。
const SHELL_KINDS: [&str; 3] = ["pwsh", "bash", "cmd"];

/// 单个标签的终端实体与状态:切标签不丢屏、不断流。
pub struct ShellTabState {
    pub view: Entity<TerminalView>,
    /// 已喂入解析器的字节(桥侧 delivered 同口径)。
    pub applied: u64,
    pub status: ShellUiStatus,
}

fn ensure_tab<'a>(
    daemon: &'a mut DaemonStore,
    shell_id: &str,
    cx: &mut Context<DaemonStore>,
) -> &'a mut ShellTabState {
    if !daemon.shell_tabs.contains_key(shell_id) {
        let req_in = daemon.requests.clone();
        let req_resize = daemon.requests.clone();
        let in_id = shell_id.to_string();
        let resize_id = shell_id.to_string();
        let view = cx.new(|cx| {
            TerminalView::new_interactive(
                format!("shell-term-{shell_id}"),
                cx.focus_handle(),
                move |bytes, _| {
                    let _ = req_in.unbounded_send(BridgeRequest::ShellStdin {
                        shell_id: in_id.clone(),
                        bytes,
                    });
                },
                move |cols, rows, _| {
                    let _ = req_resize.unbounded_send(BridgeRequest::ShellResize {
                        shell_id: resize_id.clone(),
                        cols,
                        rows,
                    });
                },
            )
        });
        daemon.shell_tabs.insert(
            shell_id.to_string(),
            ShellTabState {
                view,
                applied: 0,
                status: ShellUiStatus::Connecting,
            },
        );
    }
    daemon.shell_tabs.get_mut(shell_id).expect("tab just inserted")
}

/// 提取终端程序简名(如 `C:\...\pwsh.exe` → `pwsh`)。
fn short_shell_name(raw: &str) -> &str {
    let file = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    file.strip_suffix(".exe")
        .or_else(|| file.strip_suffix(".EXE"))
        .unwrap_or(file)
}

/// 同名档位多开带序号(electron labelOf 同律);只有一个时显示纯档位名。
fn tab_label(info: &ShellInfo, all: &[ShellInfo]) -> String {
    let short = short_shell_name(&info.shell);
    let same_kind = all
        .iter()
        .filter(|item| short_shell_name(&item.shell) == short)
        .count();
    if same_kind > 1 {
        let index = all
            .iter()
            .filter(|item| short_shell_name(&item.shell) == short)
            .position(|item| item.id == info.id)
            .map_or(same_kind, |position| position + 1);
        format!("{short} {index}")
    } else {
        short.to_owned()
    }
}

fn status_label(status: &ShellUiStatus) -> String {
    match status {
        ShellUiStatus::Connecting => "连接中…".into(),
        ShellUiStatus::Open => String::new(),
        ShellUiStatus::Reconnecting { retry_ms, reason } => {
            format!("重连中({reason}),{retry_ms}ms 后重试")
        }
        ShellUiStatus::Unsubscribed { reason } => {
            format!("订阅已停({reason}),进程仍在运行")
        }
        ShellUiStatus::Terminated { reason } => format!("已退出({reason})"),
    }
}

fn status_color(status: &ShellUiStatus, t: &Palette) -> Hsla {
    match status {
        ShellUiStatus::Connecting | ShellUiStatus::Reconnecting { .. } => t.warn,
        ShellUiStatus::Open => t.ok,
        ShellUiStatus::Unsubscribed { .. } => t.dim,
        ShellUiStatus::Terminated { .. } => t.bad,
    }
}

// ── 状态入库(BridgeMsg 分支,daemon.rs 调用)───────────────────────────

/// `shell.list` 权威全量。已消失的 shell:标签与终态屏面保留,由 ShellStatus
/// 收尾;尚未建页签态的 shell 补齐初始态并按需自动选中末项。
pub fn sync_shells(daemon: &mut DaemonStore, shells: Vec<ShellInfo>, cx: &mut Context<DaemonStore>) {
    daemon.shells = shells.clone();
    for info in &shells {
        ensure_tab(daemon, &info.id, cx);
    }
    let active_present = daemon
        .shell_active
        .as_deref()
        .is_some_and(|active| daemon.shells.iter().any(|item| item.id == active));
    if !active_present {
        daemon.shell_active = daemon.shells.last().map(|item| item.id.clone());
    }
    cx.notify();
}

/// `shell.open` 结果:追加标签、聚焦、开坞。cwd 在请求时已编好,这里只落状态。
pub fn opened(
    daemon: &mut DaemonStore,
    info: ShellInfo,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
) {
    daemon.shell_opening = None;
    daemon.shell_error = None;
    daemon.shells.retain(|item| item.id != info.id);
    daemon.shells.push(info.clone());
    let view = ensure_tab(daemon, &info.id, cx).view.clone();
    daemon.shell_active = Some(info.id);
    daemon.shell_dock_open = true;
    if let Some(kind) = SHELL_KINDS.iter().find(|kind| info.shell.starts_with(**kind)) {
        daemon.shell_last_kind = kind;
    }
    view.read(cx).focus_handle().focus(window, cx);
    cx.notify();
}

/// `shell.close` 成功:本地移除标签与终端实体,聚焦顺延到末位标签。
pub fn removed(daemon: &mut DaemonStore, shell_id: &str, cx: &mut Context<DaemonStore>) {
    daemon.shells.retain(|item| item.id != shell_id);
    daemon.shell_tabs.remove(shell_id);
    if daemon.shell_active.as_deref() == Some(shell_id) {
        daemon.shell_active = daemon.shells.last().map(|item| item.id.clone());
    }
    cx.notify();
}

/// shell ws 下行字节:喂对应页签的 `TerminalView`(后台标签照常积累,切回即得),
/// 并自动将 `PtyWrite` (`CSI 6 n` 等) 经 `on_input` 回调回写远端 ConPTY。
pub fn on_bytes(
    daemon: &mut DaemonStore,
    shell_id: &str,
    bytes: &[u8],
    cx: &mut Context<DaemonStore>,
) {
    let view = {
        let tab = ensure_tab(daemon, shell_id, cx);
        tab.applied += bytes.len() as u64;
        tab.view.clone()
    };
    view.update(cx, |view, cx| view.feed(bytes, cx));
    cx.notify();
}

/// shell ws 生命周期迁移(连接/重连/退订/终止);连接就绪时同步当前几何尺寸。
pub fn on_status(
    daemon: &mut DaemonStore,
    shell_id: &str,
    status: ShellUiStatus,
    cx: &mut Context<DaemonStore>,
) {
    let is_open = matches!(status, ShellUiStatus::Open);
    let view = {
        let tab = ensure_tab(daemon, shell_id, cx);
        tab.status = status;
        tab.view.clone()
    };
    if is_open {
        let (cols, rows) = view.read(cx).size();
        let _ = daemon.requests.unbounded_send(BridgeRequest::ShellResize {
            shell_id: shell_id.to_string(),
            cols,
            rows,
        });
    }
    cx.notify();
}

/// shell 操作/订阅失败:清除启动态、置坞内错误条并推送全局通知。
pub fn on_error(
    daemon: &mut DaemonStore,
    error: String,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
) {
    daemon.shell_opening = None;
    daemon.shell_error = Some(error.clone());
    window.push_notification(gpui_kit::component::notification::Notification::error(error), cx);
    cx.notify();
}

// ── 用户动作 ──────────────────────────────────────────────────────────────

/// 切换激活标签(顺带开坞并抢焦点:键盘/IME 输入归当前终端视口)。
pub fn activate(
    daemon: &mut DaemonStore,
    shell_id: String,
    window: &mut Window,
    cx: &mut Context<DaemonStore>,
) {
    if daemon.shell_active.as_deref() != Some(shell_id.as_str()) {
        daemon.shell_active = Some(shell_id.clone());
    }
    daemon.shell_dock_open = true;
    if let Some(view) = daemon.shell_tabs.get(&shell_id).map(|tab| tab.view.clone()) {
        view.read(cx).focus_handle().focus(window, cx);
    }
    cx.notify();
}

/// 开新终端(cwd = 当前会话工作区;kind 缺省 = 上次用的档位,Windows Terminal 同律)。
pub fn open(daemon: &mut DaemonStore, kind: &'static str, cx: &mut Context<DaemonStore>) {
    let cwd = daemon.current.as_ref().and_then(|current| {
        let seed = current.read(cx).session_id.clone();
        daemon
            .sessions
            .iter()
            .find(|row| row.id == seed)
            .and_then(|row| row.cwd.clone())
    });
    daemon.shell_last_kind = kind;
    daemon.shell_dock_open = true;
    daemon.shell_opening = Some(kind);
    daemon.shell_error = None;
    let _ = daemon.requests.unbounded_send(BridgeRequest::ShellOpen {
        cwd,
        shell: Some(kind.to_string()),
    });
    cx.notify();
}

/// 显式关标签 = 真收进程:先退订阅(桥 ShellClose 内先 drop 流)再服务面 kill。
pub fn close_tab(daemon: &mut DaemonStore, shell_id: String, cx: &mut Context<DaemonStore>) {
    let _ = daemon
        .requests
        .unbounded_send(BridgeRequest::ShellClose { shell_id: shell_id.clone() });
    removed(daemon, &shell_id, cx);
}

/// 清屏重建订阅:重置终端缓冲并重放最近尾段(electron ShellTerminal 重建口径)。
pub fn clear_screen(daemon: &mut DaemonStore, cx: &mut Context<DaemonStore>) {
    if let Some(id) = daemon.shell_active.clone() {
        let view = daemon.shell_tabs.get_mut(&id).map(|tab| {
            tab.applied = 0;
            tab.view.clone()
        });
        if let Some(view) = view {
            view.update(cx, |view, cx| view.reset(cx));
        }
        let _ = daemon
            .requests
            .unbounded_send(BridgeRequest::ShellAttach { shell_id: id });
    }
    cx.notify();
}

/// 开/收终端坞:开时刷新 shell.list 并聚焦活动终端;收时不断流(后台继续跑)。
pub fn toggle(daemon: &mut DaemonStore, window: &mut Window, cx: &mut Context<DaemonStore>) {
    daemon.shell_dock_open = !daemon.shell_dock_open;
    if daemon.shell_dock_open {
        let _ = daemon.requests.unbounded_send(BridgeRequest::ShellList);
        if let Some(view) = daemon
            .shell_active
            .as_ref()
            .and_then(|id| daemon.shell_tabs.get(id))
            .map(|tab| tab.view.clone())
        {
            view.read(cx).focus_handle().focus(window, cx);
        }
    }
    cx.notify();
}

// ── 渲染 ─────────────────────────────────────────────────────────────────

/// 终端坞:标签条 + 原生虚拟终端屏 + 状态行。物理弹簧平滑开合,关闭归零后卸载(不断流)。
pub fn dock(daemon: &mut DaemonStore, window: &mut Window, cx: &mut Context<DaemonStore>) -> Option<Div> {
    let full_height = 284.0_f32;
    let target_height = if daemon.shell_dock_open {
        full_height
    } else {
        0.0
    };
    let height = spring_f32(
        "shell-dock-height",
        target_height,
        spring_panel_policy(),
        window,
        cx,
    )
    .max(0.0);
    if !daemon.shell_dock_open && height <= 0.5 {
        return None;
    }
    let t = *cx.colors();

    // 标签条:圆角胶囊标签 + 快捷新建档位 + 收起/重建。
    let mut tabs_row = div()
        .flex()
        .items_center()
        .gap_1p5()
        .px_3()
        .py_2()
        .bg(t.surface_subtle)
        .border_b_1()
        .border_color(t.border_subtle)
        .child(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .pr_1()
                .child(
                    Icon::new(IconName::SquareTerminal)
                        .with_size(Size::XSmall)
                        .text_color(t.accent),
                )
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child("终端"),
                ),
        );
    for info in daemon.shells.clone() {
        let active = daemon.shell_active.as_deref() == Some(info.id.as_str());
        let label = tab_label(&info, &daemon.shells);
        let tab_id = info.id.clone();
        let tab_status = daemon
            .shell_tabs
            .get(&info.id)
            .map(|tab| tab.status.clone())
            .unwrap_or(ShellUiStatus::Connecting);
        let is_open = matches!(tab_status, ShellUiStatus::Open);
        let mut chip = div()
            .id(SharedString::from(format!("shell-tab-{}", info.id)))
            .flex()
            .items_center()
            .gap_1p5()
            .px_2p5()
            .py_1()
            .rounded(px(RADIUS_CHIP))
            .text_xs()
            .font_weight(if active {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .cursor_pointer()
            .text_color(if active { t.text } else { t.dim })
            .when(active, |el| {
                el.bg(t.card)
                    .border_1()
                    .border_color(t.border_subtle)
            })
            .when(!active, |el| el.hover(|style| style.bg(t.hover)))
            .on_click(cx.listener(move |daemon, _, window, cx| {
                activate(daemon, tab_id.clone(), window, cx);
            }))
            .child(live_dot(
                format!("shell-{}", info.id),
                status_color(&tab_status, &t),
                is_open,
            ))
            .child(label);
        let close_id = info.id.clone();
        chip = chip.child(
            div()
                .id(SharedString::from(format!("shell-tab-close-{}", info.id)))
                .size(px(16.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_color(t.dim)
                .hover(|style| style.bg(soft(t.bad, 0.14)).text_color(t.bad))
                .on_click(cx.listener(move |daemon, _, _, cx| {
                    cx.stop_propagation();
                    close_tab(daemon, close_id.clone(), cx);
                }))
                .child(Icon::new(IconName::X).with_size(Size::XSmall)),
        );
        tabs_row = tabs_row.child(chip);
    }
    for kind in SHELL_KINDS {
        tabs_row = tabs_row.child(
            div()
                .id(SharedString::from(format!("shell-open-{kind}")))
                .px_2()
                .py_1()
                .rounded(px(RADIUS_CHIP))
                .bg(t.btn)
                .text_xs()
                .cursor_pointer()
                .text_color(t.dim)
                .hover(|style| style.text_color(t.text).bg(t.btn_hover))
                .on_click(cx.listener(move |daemon, _, _, cx| open(daemon, kind, cx)))
                .child(SharedString::from(format!("+ {kind}"))),
        );
    }
    tabs_row = tabs_row
        .child(div().flex_1())
        .child(
            div()
                .id("shell-clear")
                .px_2()
                .py_1()
                .rounded(px(RADIUS_CHIP))
                .text_xs()
                .cursor_pointer()
                .text_color(t.dim)
                .hover(|style| style.text_color(t.text).bg(t.hover))
                .on_click(cx.listener(|daemon, _, _, cx| clear_screen(daemon, cx)))
                .child("清屏重建"),
        )
        .child(
            div()
                .id("shell-hide")
                .px_2()
                .py_1()
                .rounded(px(RADIUS_CONTROL))
                .text_xs()
                .cursor_pointer()
                .text_color(t.dim)
                .hover(|style| style.text_color(t.text).bg(t.hover))
                .on_click(cx.listener(|daemon, _, _, cx| {
                    daemon.shell_dock_open = false;
                    cx.notify();
                }))
                .child("收起"),
        );

    let term = terminal_palette(settings::get(cx).appearance.terminal_scheme, &t);
    let active_view = daemon
        .shell_active
        .as_ref()
        .and_then(|id| daemon.shell_tabs.get(id))
        .map(|tab| tab.view.clone());
    let status = daemon
        .shell_active
        .as_ref()
        .and_then(|id| daemon.shell_tabs.get(id))
        .map(|tab| status_label(&tab.status))
        .unwrap_or_default();
    let shell_error = daemon.shell_error.clone();
    let opening_kind = daemon.shell_opening;
    let conn_detail = match &daemon.connection {
        ConnectionState::Failed { detail } => Some(detail.clone()),
        _ => None,
    };
    let conn_connecting = matches!(daemon.connection, ConnectionState::Connecting);

    let mut body = div()
        .flex()
        .flex_col()
        .h(px(full_height))
        .min_h_0()
        .rounded(px(RADIUS_STAGE))
        .bg(t.stage)
        .border_1()
        .border_color(t.border_subtle)
        .shadow(shadow_stage(&t))
        .overflow_hidden()
        .child(tabs_row);

    if let Some(error) = shell_error {
        let dismiss = cx.listener(|daemon, _, _, cx| {
            daemon.shell_error = None;
            cx.notify();
        });
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .bg(soft(t.bad, 0.10))
                .border_b_1()
                .border_color(soft(t.bad, 0.25))
                .child(
                    Icon::new(IconName::TriangleAlert)
                        .with_size(Size::XSmall)
                        .text_color(t.bad),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .text_color(t.bad)
                        .child(SharedString::from(error)),
                )
                .child(
                    div()
                        .id("shell-error-dismiss")
                        .size(px(18.))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .text_color(t.bad)
                        .hover(|style| style.bg(soft(t.bad, 0.16)))
                        .on_click(dismiss)
                        .child(Icon::new(IconName::X).with_size(Size::XSmall)),
                ),
        );
    }

    if let Some(active_view) = active_view {
        let focus_view = active_view.clone();
        body = body.child(
            div()
                .id("shell-screen")
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .m_2p5()
                .rounded(px(RADIUS_CARD))
                .bg(term.bg)
                .border_1()
                .border_color(t.border_subtle)
                .p_2()
                .overflow_hidden()
                .on_click(move |_, window, cx| {
                    focus_view.read(cx).focus_handle().focus(window, cx);
                })
                .child(active_view),
        );
    } else if let Some(kind) = opening_kind {
        body = body.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_xs()
                .text_color(t.dim)
                .child(live_dot("shell-opening", t.accent, true))
                .child(SharedString::from(format!("正在启动 {kind}…"))),
        );
    } else if let Some(detail) = conn_detail {
        let retry_listener = cx.listener(|daemon, _, _, cx| {
            daemon.shell_error = None;
            let _ = daemon.requests.unbounded_send(BridgeRequest::Reconnect);
            cx.notify();
        });
        body = body.child(
            div()
                .flex_1()
                .px_6()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_2()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.text)
                        .child(
                            Icon::new(IconName::TriangleAlert)
                                .with_size(Size::XSmall)
                                .text_color(t.bad),
                        )
                        .child("daemon 未连接，无法启动交互终端"),
                )
                .child(
                    div()
                        .max_w(px(640.))
                        .text_xs()
                        .text_color(t.dim)
                        .text_center()
                        .child(SharedString::from(detail)),
                )
                .child(button(
                    cx,
                    BtnKind::Soft,
                    "shell-reconnect",
                    "重试连接",
                    &t,
                    retry_listener,
                )),
        );
    } else if conn_connecting {
        body = body.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .gap_2()
                .text_xs()
                .text_color(t.dim)
                .child(live_dot("shell-conn", t.warn, true))
                .child("正在连接本地 daemon，就绪后即可启动终端…"),
        );
    } else {
        body = body.child(
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(t.dim)
                .child("暂无交互终端 —— 点击上方 + pwsh / + bash / + cmd 启动。"),
        );
    }
    if !status.is_empty() {
        body = body.child(
            div()
                .px_3()
                .pb_2()
                .text_xs()
                .text_color(t.dim)
                .child(SharedString::from(status)),
        );
    }
    let ratio = (height / full_height).clamp(0.0, 1.0);
    Some(
        div()
            .h(px(height))
            .mt(px(8.0 * ratio))
            .opacity(ratio)
            .flex_shrink_0()
            .overflow_hidden()
            .child(body),
    )
}
