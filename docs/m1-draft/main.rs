//! qaqh-gpui — QAQ-Harness 的 GPUI 桌面前端(RingingV2 客户端)。
//!
//! `--probe` 以无 GUI 方式跑连接序列(discovery → open_v2 → session.list),
//! 用于环境验证;默认启动窗口。

mod app;
mod bridge;
mod reducer;
mod store;
mod ui;

fn main() {
    if std::env::args().any(|arg| arg == "--probe") {
        bridge::probe();
        return;
    }
    app::launch();
}
