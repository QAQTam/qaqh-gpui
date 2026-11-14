fn main() {
    // Windows:嵌入应用图标(app.ico)。host==target 的 native 构建(与 build-release.sh 口径一致),
    // 因此 cfg(target_os) 与 build-dependencies 的 target 门控一致;不假设交叉编译。
    #[cfg(target_os = "windows")]
    embed_resource::compile(
        "resources/windows/app.rc",
        // app.rc 引用仓库根的 packaging/app.ico;include dirs 指向 workspace 根。
        embed_resource::ParamsIncludeDirs(["../.."]),
    )
    .manifest_optional()
    .expect("app.rc icon compilation failed");

    // 版本固定来自 Cargo.toml(4.0.0),不再拼接 git 哈希/日期。
    println!("cargo:rustc-env=QAQH_VERSION={}", env!("CARGO_PKG_VERSION"));
}
