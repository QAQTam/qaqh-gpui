//! GPUI 壳(烟雾版):一个窗口滚动显示 daemon 连接状态行。

use futures::StreamExt;
use gpui::App;
use gpui::AppContext as _;
use gpui::Bounds;
use gpui::Context;
use gpui::Entity;
use gpui::Render;
use gpui::TitlebarOptions;
use gpui::Window;
use gpui::WindowBounds;
use gpui::WindowOptions;
use gpui::div;
use gpui::prelude::*;
use gpui::px;
use gpui::rgb;
use gpui::size;

struct StatusLog {
    lines: Vec<String>,
}

impl Render for StatusLog {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x10141a))
            .text_color(rgb(0xd8dee9))
            .flex()
            .flex_col()
            .gap_1()
            .p_4()
            .overflow_y_scroll()
            .children(self.lines.iter().map(|line| div().child(line.clone())))
    }
}

struct ShellView {
    log: Entity<StatusLog>,
}

impl Render for ShellView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(self.log.clone())
    }
}

pub fn launch() {
    gpui_platform::application().run(|cx: &mut App| {
        let status = cx.new(|_| StatusLog {
            lines: vec!["qaqh-gpui 启动…".into()],
        });

        let (sender, mut receiver) = futures::channel::mpsc::unbounded::<String>();
        std::thread::Builder::new()
            .name("qaqh-bridge".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                runtime.block_on(crate::runtime::run_connection(move |line| {
                    let _ = sender.unbounded_send(line);
                }));
            })
            .expect("spawn bridge thread");

        cx.spawn(async move |cx| {
            while let Some(line) = receiver.next().await {
                status.update(cx, |log, cx| {
                    log.lines.push(line);
                    cx.notify();
                })?;
            }
            anyhow::Ok(())
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(720.), px(480.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("QAQ Harness".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| {
                cx.new(|_| ShellView {
                    log: status.clone(),
                })
            },
        )
        .expect("open window");

        cx.activate(true);
    });
}
