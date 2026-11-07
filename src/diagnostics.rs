//! Allowlisted diagnostics: never record message bodies, URLs, tokens or settings.
use std::{io::Write, path::PathBuf, sync::Mutex};
static LOG: Mutex<()> = Mutex::new(());
static EPOCH: Mutex<Option<String>> = Mutex::new(None);

pub fn set_epoch(epoch: String) {
    *EPOCH.lock().expect("epoch lock") = Some(epoch);
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
                "epoch":*EPOCH.lock().expect("epoch lock"),"command_revision":qaqh_client::ringing::COMMAND_CONTRACT_REVISION,"session_revision":qaqh_client::ringing::SESSION_CONTRACT_REVISION})
        )
    })();
    if let Err(error) = result {
        eprintln!("diagnostics write failed: {error}");
    }
}
pub fn report() -> String {
    format!(
        "QAQHarness GPUI {}\nrevision: {}\ncommands: {}\nsession: {}\nplatform: {} / {}\nlogs: {}\n{}",
        env!("CARGO_PKG_VERSION"),
        env!("QAQH_BUILD_REVISION"),
        qaqh_client::ringing::COMMAND_CONTRACT_REVISION,
        qaqh_client::ringing::SESSION_CONTRACT_REVISION,
        std::env::consts::OS,
        std::env::consts::ARCH,
        path().display(),
        std::fs::read_to_string(path()).unwrap_or_default()
    )
}
