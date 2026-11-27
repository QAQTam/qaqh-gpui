//! GPUI 壳(M1):窗口根 = DaemonStore 实体;桥线程 + 前台消息泵。

use std::time::Duration;

use futures::StreamExt;
use gpui_kit::App;
use gpui_kit::AppContext as _;
use gpui_kit::Bounds;
use gpui_kit::Entity;
use gpui_kit::Render;
use gpui_kit::Window;
use gpui_kit::WindowBackgroundAppearance;
use gpui_kit::WindowBounds;
use gpui_kit::div;
use gpui_kit::prelude::*;
use gpui_kit::px;
use gpui_kit::size;

use qaqh_gpui_wire::bridge;
use qaqh_gpui_wire::bridge::BridgeMsg;
use qaqh_gpui_wire::bridge::BridgeRequest;
use qaqh_gpui_shell::daemon::DaemonStore;

struct RootShell {
    daemon: Entity<DaemonStore>,
}

impl Render for RootShell {
    fn render(&mut self, window: &mut Window, cx: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        let bounds = window.bounds();
        self.daemon.update(cx, |daemon, _| {
            daemon.workbench.window = Some([f32::from(bounds.origin.x), f32::from(bounds.origin.y), f32::from(bounds.size.width), f32::from(bounds.size.height)]);
            daemon.workbench.maximized = matches!(window.window_bounds(), WindowBounds::Maximized(_));
        });
        div().size_full().child(self.daemon.clone())
    }
}

pub fn launch() {
    gpui_kit::application()
        // Assets 仅含组件库最小子集(约 60 个),会导致业务侧图标静默空白;
        // AppAssets 优先内嵌彩色语言/工具品牌 SVG,其余全部回退完整 Lucide AllAssets。
        .with_assets(qaqh_gpui_kit::icons::AppAssets)
        .run(|cx: &mut App| {
        cx.set_app_identity("io.github.QAQTam.QAQHarness", "QAQHarness");
        // 顺序:settings 先于 theme(theme::init 读设置的主题模式);
        // gpui_kit::init 建组件库全局(Theme/输入法/弹出层),theme::init 随后映射品牌色。
        qaqh_gpui_kit::settings::init(cx);
        gpui_kit::init(cx);
        qaqh_gpui_kit::theme::init(cx);
        let (to_ui, mut from_bridge) = futures::channel::mpsc::unbounded::<BridgeMsg>();
        let (request_tx, request_rx) = futures::channel::mpsc::unbounded::<BridgeRequest>();
        // 窗口先行:InputState/TextareaState 只能在窗口上下文构造,DaemonStore
        // 随窗口诞生;持久化的窗口几何在开窗前读一次,开窗回调只消费。
        let (workbench, load_error) = match qaqh_gpui_session::persist::load() {
            Ok(state) => (state, None),
            Err(error) => (Default::default(), Some(format!("工作状态读取失败：{error}"))),
        };
        let bounds = workbench.window.map(|b| Bounds {
            origin: gpui_kit::point(px(b[0]), px(b[1])), size: size(px(b[2]), px(b[3])),
        }).unwrap_or_else(|| Bounds::centered(None, size(px(1240.), px(780.)), cx));
        let maximized = workbench.maximized;
        let mut window_options = gpui_kit::component::TitleBar::window_options();
        window_options.window_bounds = Some(if maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        });
        window_options.window_min_size = Some(size(px(860.), px(560.)));
        window_options.window_background = WindowBackgroundAppearance::MicaBackdrop;
        if let Some(titlebar) = window_options.titlebar.as_mut() {
            titlebar.title = Some("QAQ Harness".into());
        }
        let daemon_slot = std::cell::RefCell::new(None);
        let window_handle = cx.open_window(
            window_options,
            |window, cx| {
                window.set_app_id("io.github.QAQTam.QAQHarness");
                let daemon = cx.new(|cx| DaemonStore::new(request_tx, workbench, load_error, window, cx));
                window.on_window_should_close(cx, {
                    let closing = daemon.clone();
                    move |window, cx| {
                        closing.update(cx, |daemon, cx| {
                            let bounds = window.bounds();
                            daemon.workbench.window = Some([f32::from(bounds.origin.x), f32::from(bounds.origin.y), f32::from(bounds.size.width), f32::from(bounds.size.height)]);
                            daemon.workbench.maximized = matches!(window.window_bounds(), WindowBounds::Maximized(_));
                            daemon.persist(cx);
                        });
                        true
                    }
                });
                *daemon_slot.borrow_mut() = Some(daemon.clone());
                cx.new(|cx| {
                    // 系统亮/暗切换 → 重选色板并刷新窗口(M1)。
                    cx.observe_window_appearance(window, |_, window, cx| {
                        qaqh_gpui_kit::theme::system_appearance_changed(window.appearance(), cx);
                    })
                    .detach();
                    // Root(gpui-base)承载组件库的 popover/menu/notification 窗口级层;
                    // RootShell 作为内容视图,保留原有的窗口几何记账。
                    let shell: Entity<RootShell> = cx.new(|_cx| RootShell { daemon: daemon.clone() });
                    gpui_kit::base::Root::new(shell, window, cx)
                })
            },
        )
        .expect("open window");
        let daemon = daemon_slot.into_inner().expect("开窗回调必已构造 DaemonStore");

        // 系统通知点击 → 定位会话:select 要建会话实体,经窗口上下文路由。
        let notifications = daemon.clone();
        // 必须 Deref 出 AnyWindowHandle:WindowHandle::update 会整段闭包独占 lease
        // 窗口根(gpui_base::Root),闭包内组件库 Root API(open_dialog /
        // push_notification)read 根必炸;AnyWindowHandle::update 只进窗口上下文。
        let notify_window = *window_handle;
        cx.on_system_notification_response(move |response, cx| {
            if let Some(seed) = response.tag.strip_prefix("qaqh-") {
                let Ok(seed) = qaqh_core_vocab::SessionId::new(seed) else {
                    return;
                };
                let _ = notify_window.update(cx, |_, window, cx| {
                    notifications.update(cx, |daemon, cx| {
                        daemon.section = qaqh_gpui_shell::daemon::PrimarySection::Home;
                        daemon.settings_open = false;
                        daemon.new_session_open = false;
                        daemon.select(seed, window, cx);
                    });
                    cx.activate(true);
                });
            }
        });

        let saving = daemon.clone();
        let save_window = *window_handle;
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                // 验收驱动(acceptance::poll 写 composer)与持久化在窗口上下文内执行;
                // 窗口已关 = 应用退出中,循环收尾。
                if save_window.update(cx, |_, _window, cx| {
                    saving.update(cx, |daemon, cx| {
                        #[cfg(feature = "acceptance")]
                        crate::acceptance::poll(daemon, _window, cx);
                        daemon.persist(cx);
                        // 浏览器登录流程:2s 轮询 sign_in.status 至终态;团队页可见时低频轮询。
                        daemon.poll_sign_in();
                        daemon.poll_team_page();
                        for store in &daemon.stores {
                            let store = store.read(cx);
                            if store.snapshot_pending && matches!(store.stream, qaqh_gpui_session::store::StreamState::Open) {
                                let _ = store.requests().unbounded_send(BridgeRequest::Refill(store.session_id.clone()));
                            }
                        }
                    });
                }).is_err() {
                    break;
                }
            }
        }).detach();

        let client_instance_id = daemon.read(cx).workbench.client_instance_id.clone();
        std::thread::Builder::new()
            .name("qaqh-bridge".into())
            .spawn(move || bridge::run(to_ui, request_rx, client_instance_id))
            .expect("spawn bridge thread");

        let pumping = daemon.clone();
        let pump_window = *window_handle;
        cx.spawn(async move |cx| {
            while let Some(message) = from_bridge.next().await {
                // Yield to input and paint even while the bridge queue stays ready.
                cx.background_executor().timer(Duration::from_millis(16)).await;
                // 桥消息在窗口上下文内处理:新会话实体/输入框/ask 草稿都要窗口。
                if pump_window.update(cx, |_, window, cx| {
                    pumping.update(cx, |store, cx| {
                        store.handle(message, window, cx);
                        for _ in 1..128 {
                            let Ok(message) = from_bridge.try_recv() else {
                                break;
                            };
                            store.handle(message, window, cx);
                        }
                    });
                }).is_err() {
                    break;
                }
            }
            // 通道关闭 = 桥线程已退出(panic/崩溃):把连接态标为失败,
            // 界面立刻显示原因,不再停在「正在连接」的假等待。
            let _ = pumping.update(cx, |daemon, cx| {
                daemon.connection = qaqh_gpui_wire::bridge::ConnectionState::Failed {
                    detail: "桥线程已退出".into(),
                };
                cx.notify();
            });
        })
        .detach();

        cx.activate(true);
    });
}
