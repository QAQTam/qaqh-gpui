//! qaqh-gpui — QAQ-Harness 的 GPUI 桌面前端(RingingV2 客户端)。
//!
//! `--probe` 以无 GUI 方式跑连接序列(discovery → open_v2 → session.list),
//! 用于环境验证;默认启动窗口。

mod app;
#[cfg(feature = "acceptance")]
mod acceptance;
mod ask;
mod bridge;
mod code_panel;
mod delivery;
mod desktop;
mod content;
mod sidebar;
mod diagnostics;
mod workbench;
mod highlight;
mod markdown;
mod media;
mod reducer;
mod settings;
mod settings_ui;
mod store;
mod text_input;
mod theme;
mod ui;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => app::launch(),
        [flag] if flag == "--version" => println!("qaqh-gpui {} ({}){}", env!("CARGO_PKG_VERSION"), env!("QAQH_BUILD_REVISION"), if cfg!(feature = "acceptance") { " [acceptance]" } else { "" }),
        [flag] if flag == "--diagnostics" => println!("{}", diagnostics::report()),
        [flag] if flag == "--probe" => bridge::probe(),
        [flag] if flag == "--help" => println!("QAQHarness native desktop client\n\nqaqh-gpui                Open the desktop window\nqaqh-gpui --probe        Check daemon connectivity\nqaqh-gpui --diagnostics  Export redacted diagnostics\nqaqh-gpui --version      Show build information"),
        #[cfg(feature = "acceptance")]
        [flag, _root] if flag == "--acceptance-root" => app::launch(),
        _ => { eprintln!("Unknown arguments; use qaqh-gpui --help"); std::process::exit(2); },
    }
}
