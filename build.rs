fn main() {
    // Windows:嵌入应用图标(app.ico)。host==target 的 native 构建(与 build-release.sh 口径一致),
    // 因此 cfg(target_os) 与 build-dependencies 的 target 门控一致;不假设交叉编译。
    #[cfg(target_os = "windows")]
    embed_resource::compile(
        "resources/windows/app.rc",
        embed_resource::ParamsIncludeDirs(["."]),
    )
    .manifest_optional()
    .expect("app.rc icon compilation failed");

    println!("cargo:rerun-if-env-changed=QAQH_BUILD_REVISION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    let revision = match std::env::var("QAQH_BUILD_REVISION") {
        Ok(revision) => revision,
        Err(_) => {
            let revision = std::process::Command::new("git")
                .args(["rev-parse", "--short=12", "HEAD"])
                .output()
                .expect("git required for build provenance");
            assert!(revision.status.success(), "build must have git provenance");
            let dirty = std::process::Command::new("git")
                .args(["status", "--porcelain"])
                .output()
                .expect("git status");
            format!(
                "{}{}",
                String::from_utf8_lossy(&revision.stdout).trim(),
                if dirty.stdout.is_empty() {
                    ""
                } else {
                    "-dirty"
                }
            )
        }
    };
    println!("cargo:rustc-env=QAQH_BUILD_REVISION={revision}");
    // 展示版本:基线(Cargo.toml 3.0.YYYYMMDD)-g{哈希};-dirty 只进 provenance,
    // 不污染版本号本身。哈希现算,提交后自动跟随,无需手改 Cargo.toml。
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    println!(
        "cargo:rustc-env=QAQH_VERSION={version}-g{}",
        revision.trim_end_matches("-dirty")
    );
}
