// Windows release 以图形子系统链接:GUI 启动不再分配控制台(Win11 默认宿主
// Windows Terminal 会随主窗口弹出)。debug 保留控制台便于看日志;
// CLI 口径经 attach_parent_console 挂回调用方终端。
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]

//! qaqh-gpui — QAQ-Harness 的 GPUI 桌面前端(RingingV3 客户端)。
//!
//! `--probe` 以无 GUI 方式跑连接序列(discovery → V3 握手 → session.list),
//! 用于环境验证;默认启动窗口。

mod app;
#[cfg(feature = "acceptance")]
mod acceptance;

fn main() {
    // --probe/--version/--diagnostics/--help 仍需在终端输出:父进程有控制台则
    // 挂回,双击启动(父进程非控制台)时挂接失败,无副作用。必须在首次 println 前。
    #[cfg(all(target_os = "windows", not(debug_assertions)))]
    attach_parent_console();

    // 构建期版本注入 wire 诊断面(build.rs env 只对 bin 可见)。
    qaqh_gpui_wire::diagnostics::init(env!("QAQH_VERSION"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => app::launch(),
        [flag] if flag == "--version" => println!("qaqh-gpui {}{}", env!("QAQH_VERSION"), if cfg!(feature = "acceptance") { " [acceptance]" } else { "" }),
        [flag] if flag == "--diagnostics" => println!("{}", qaqh_gpui_wire::diagnostics::report()),
        [flag] if flag == "--probe" => qaqh_gpui_wire::bridge::probe(),
        [flag] if flag == "--help" => println!("QAQHarness native desktop client\n\nqaqh-gpui                Open the desktop window\nqaqh-gpui --probe        Check daemon connectivity\nqaqh-gpui --diagnostics  Export redacted diagnostics\nqaqh-gpui --version      Show build information"),
        #[cfg(feature = "acceptance")]
        [flag, _root] if flag == "--acceptance-root" => app::launch(),
        _ => { eprintln!("Unknown arguments; use qaqh-gpui --help"); std::process::exit(2); },
    }
}

/// 挂接父进程控制台,使图形子系统的 release 仍能用 CLI 口径输出。
#[cfg(all(target_os = "windows", not(debug_assertions)))]
fn attach_parent_console() {
    unsafe extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    // ATTACH_PARENT_PROCESS = (DWORD)-1。
    unsafe { AttachConsole(u32::MAX) };
}
