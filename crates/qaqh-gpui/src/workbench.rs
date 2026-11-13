//! Local workbench persistence, deliberately separate from daemon business state.
use crate::delivery::{DeliveryState, Outbox};
use qaqh_client::{ContentRef, ImageBlock};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::PathBuf};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Draft {
    pub text: String,
    pub images: Vec<ImageBlock>,
    pub attachments: Vec<ContentRef>,
    pub outbox: Option<Outbox>,
    pub local_root: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
pub struct Workbench {
    #[serde(default = "new_client_instance_id")]
    pub client_instance_id: String,
    pub recent_session: Option<String>,
    pub drafts: BTreeMap<String, Draft>,
    pub window: Option<[f32; 4]>,
    pub maximized: bool,
    pub sidebar_width: Option<f32>,
    pub collapsed: Vec<String>,
}

fn new_client_instance_id() -> String { qaqh_ringing_contract::ulid::generate_ulid() }

impl Default for Workbench {
    fn default() -> Self {
        Self { client_instance_id: new_client_instance_id(), recent_session: None,
            drafts: BTreeMap::new(), window: None, maximized: false, sidebar_width: None, collapsed: Vec::new() }
    }
}

fn path() -> PathBuf {
    crate::settings::config_path().with_file_name("workbench.json")
}

pub fn load() -> Result<Workbench, String> {
    match std::fs::read(path()) {
        Ok(bytes) => {
            let mut state: Workbench = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            for draft in state.drafts.values_mut() {
                if let Some(outbox) = &mut draft.outbox
                    && outbox.state == DeliveryState::Sending
                {
                    outbox.state = DeliveryState::Uncertain;
                }
            }
            save(&state)?;
            Ok(state)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let state = Workbench::default();
            save(&state)?;
            Ok(state)
        },
        Err(e) => Err(e.to_string()),
    }
}

pub fn save(state: &Workbench) -> Result<(), String> {
    let path = path();
    std::fs::create_dir_all(path.parent().expect("workbench parent")).map_err(|e| e.to_string())?;
    let text = serde_json::to_vec(state).map_err(|e| e.to_string())?;
    if std::fs::read(&path).ok().as_deref() == Some(text.as_slice()) {
        return Ok(());
    }
    let temp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    file.write_all(&text)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}
