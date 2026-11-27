//! Allowlisted diagnostics: never record message bodies, URLs, tokens or settings.
//!
//! RingingV3 口径:诊断里只允许出现**协议自证字段**(`protocol_rev` + 协议指纹 +
//! daemon 构建号),不再有 V2 的 command/session contract revision。
use std::fmt::Write as _;
use std::{io::Write, path::PathBuf, sync::Mutex, sync::OnceLock};
static LOG: Mutex<()> = Mutex::new(());
static EPOCH: Mutex<Option<String>> = Mutex::new(None);
static BUILD_ID: Mutex<Option<String>> = Mutex::new(None);
static VERSION: OnceLock<&'static str> = OnceLock::new();

/// bin 启动时注入构建期版本(build.rs 的 rustc-env 只对 bin 可见)。
pub fn init(version: &'static str) {
    VERSION.set(version).expect("diagnostics::init once at startup");
}

pub fn version() -> &'static str {
    VERSION.get().expect("diagnostics::init before use")
}

/// 记录握手得到的服务端实例 id(诊断面唯一允许的会话标识)。
pub fn set_epoch(epoch: String) {
    *EPOCH.lock().expect("epoch lock") = Some(epoch);
}

/// 记录 `daemon.endpoint` 里的构建号(端点是 postcard 强类型,构建号是它的字段)。
pub fn set_build_id(build_id: String) {
    *BUILD_ID.lock().expect("build id lock") = Some(build_id);
}

/// 协议指纹(32 字节 → 64 位小写 hex)。
fn fingerprint_hex() -> String {
    let mut out = String::with_capacity(64);
    for byte in qaqh_ringing_v3::PROTOCOL_FINGERPRINT {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn build_id() -> String {
    BUILD_ID.lock().expect("build id lock").clone().unwrap_or_default()
}

pub fn path() -> PathBuf {
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        });
    root.join("qaqh-gpui/events.jsonl")
}
pub fn record(event: &str, session: Option<&str>, command: Option<&str>, seq: Option<u64>) {
    let Ok(_guard) = LOG.lock() else {
        return;
    };
    let path = path();
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().expect("log parent"))?;
        if path
            .metadata()
            .map(|m| m.len() > 1_048_576)
            .unwrap_or(false)
        {
            std::fs::rename(&path, path.with_extension("previous.jsonl"))?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        writeln!(
            file,
            "{}",
            serde_json::json!({"time":timestamp,"event":event,"session":session,"command":command,"seq":seq,
                "server_instance":*EPOCH.lock().expect("epoch lock"),"build_id":build_id(),
                "protocol_rev":qaqh_ringing_v3::PROTOCOL_REV,"protocol_fingerprint":fingerprint_hex()})
        )
    })();
    if let Err(error) = result {
        eprintln!("diagnostics write failed: {error}");
    }
}
pub fn report() -> String {
    format!(
        "QAQHarness GPUI {}\nprotocol_rev: {}\nprotocol_fingerprint: {}\ndaemon build_id: {}\nserver instance: {}\nplatform: {} / {}\nlogs: {}\n{}",
        version(),
        qaqh_ringing_v3::PROTOCOL_REV,
        fingerprint_hex(),
        build_id(),
        EPOCH.lock().expect("epoch lock").clone().unwrap_or_default(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        path().display(),
        std::fs::read_to_string(path()).unwrap_or_default()
    )
}
