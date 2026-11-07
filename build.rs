fn main() {
    println!("cargo:rerun-if-env-changed=QAQH_BUILD_REVISION");
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    if let Ok(revision) = std::env::var("QAQH_BUILD_REVISION") {
        println!("cargo:rustc-env=QAQH_BUILD_REVISION={revision}");
        return;
    }
    let revision = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .expect("git required for build provenance");
    assert!(revision.status.success(), "build must have git provenance");
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .expect("git status");
    println!(
        "cargo:rustc-env=QAQH_BUILD_REVISION={}{}",
        String::from_utf8_lossy(&revision.stdout).trim(),
        if dirty.stdout.is_empty() {
            ""
        } else {
            "-dirty"
        }
    );
}
