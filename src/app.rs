//! GPUI 壳(M1):窗口根 = DaemonStore 实体;桥线程 + 前台消息泵。

use std::time::Duration;

use futures::StreamExt;
use gpui::App;
use gpui::AppContext as _;
use gpui::Bounds;
use gpui::Entity;
use gpui::Render;
use gpui::TitlebarOptions;
use gpui::Window;
use gpui::WindowBounds;
use gpui::WindowOptions;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::size;

use crate::bridge;
use crate::bridge::BridgeMsg;
use crate::bridge::BridgeRequest;
use crate::store::DaemonStore;

struct RootShell {
    daemon: Entity<DaemonStore>,
}

impl Render for RootShell {
    fn render(&mut self, _window: &mut Window, _cx: &mut gpui::Context<Self>) -> impl IntoElement {
        div().size_full().child(self.daemon.clone())
    }
}

pub fn launch() {
    gpui_platform::application().run(|cx: &mut App| {
        // 顺序:settings 先于 theme(theme::init 读设置的主题模式)。
        crate::settings::init(cx);
        crate::theme::init(cx);
        let (to_ui, mut from_bridge) = futures::channel::mpsc::unbounded::<BridgeMsg>();
        let (request_tx, request_rx) = futures::channel::mpsc::unbounded::<BridgeRequest>();
        let daemon = cx.new(|cx| DaemonStore::new(request_tx, cx));
        let root = daemon.clone();

        std::thread::Builder::new()
            .name("qaqh-bridge".into())
            .spawn(move || bridge::run(to_ui, request_rx))
            .expect("spawn bridge thread");

        cx.spawn(async move |cx| {
            while let Some(message) = from_bridge.next().await {
                // Yield to input and paint even while the bridge queue stays ready.
                cx.background_executor().timer(Duration::from_millis(16)).await;
                daemon.update(cx, |store, cx| {
                    store.handle(message, cx);
                    for _ in 1..128 {
                        let Ok(message) = from_bridge.try_recv() else {
                            break;
                        };
                        store.handle(message, cx);
                    }
                });
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1100.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("QAQ Harness".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    // 系统亮/暗切换 → 重选色板并刷新窗口(M1)。
                    cx.observe_window_appearance(window, |_, window, cx| {
                        crate::theme::system_appearance_changed(window.appearance(), cx);
                    })
                    .detach();
                    RootShell { daemon: root.clone() }
                })
            },
        )
        .expect("open window");

        cx.activate(true);
    });
}
