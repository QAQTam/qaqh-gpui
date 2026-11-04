//! qaqh-gpui — QAQ-Harness 的 GPUI 桌面前端(RingingV2 客户端)。
//!
//! 烟雾版:窗口滚动显示 daemon 发现 → v2 open → session.list 的状态行;
//! `--probe` 以无 GUI 方式跑同一条连接序列,用于环境验证。

mod app;
mod runtime;

fn main() {
    if std::env::args().any(|arg| arg == "--probe") {
        runtime::probe();
        return;
    }
    app::launch();
}
