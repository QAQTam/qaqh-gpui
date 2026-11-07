//! One native integration driver, compiled only with --features acceptance.
//! Uses the real stores, bridge, SDK and rendered window against an isolated daemon.
use crate::{
    bridge::{BridgeRequest, ConnectionState},
    store::{DaemonStore, StreamState},
    text_input::TextField,
};
use gpui::Context;
use serde::Deserialize;
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
#[derive(Deserialize)]
struct Control {
    sequence: u64,
    #[serde(flatten)]
    action: Action,
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Action {
    Select {
        session_id: String,
    },
    Draft {
        text: String,
    },
    Send,
    FailSend,
    Retry,
    Receipt,
    Reconnect,
    Refill,
    LongSession {
        entries: usize,
    },
    Driver {
        release: bool,
        stale_holder: Option<String>,
    },
    Attach {
        path: PathBuf,
    },
    Settings,
    Workspaces,
    Welcome,
}

pub fn poll(daemon: &mut DaemonStore, cx: &mut Context<DaemonStore>) {
    let args: Vec<_> = std::env::args().collect();
    let Some(index) = args.iter().position(|arg| arg == "--acceptance-root") else {
        return;
    };
    let root = PathBuf::from(args.get(index + 1).expect("acceptance root"));
    assert!(root.is_absolute());
    assert_eq!(
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        Some(root.join("config")),
        "acceptance requires isolated config"
    );
    let mut error = None;
    if let Ok(bytes) = std::fs::read(root.join("control.json")) {
        match serde_json::from_slice::<Control>(&bytes) {
            Ok(control) if control.sequence > SEQUENCE.load(Ordering::Relaxed) => {
                SEQUENCE.store(control.sequence, Ordering::Relaxed);
                match control.action {
                    Action::Select { session_id } => daemon.select(session_id, cx),
                    Action::Reconnect => {
                        let _ = daemon.requests.unbounded_send(BridgeRequest::Reconnect);
                    }
                    Action::Settings => {
                        daemon.settings_open = true;
                        daemon.new_session_open = false;
                    }
                    Action::Workspaces => {
                        daemon.settings_open = true;
                        daemon.settings_editor = crate::store::SettingsEditor::Workspaces;
                    }
                    Action::Welcome => {
                        daemon.current = None;
                        daemon.settings_open = false;
                    }
                    action => {
                        if let Some(store) = daemon.current.clone() {
                            store.update(cx, |store, cx| {
                            match action {
                                Action::Draft { text } => store.composer = TextField::with_text(&text),
                                Action::Send => { if let Some(reason) = store.composer_send() { error = Some(reason.to_string()); } },
                                Action::FailSend => store.simulate_closed_bridge(),
                                Action::Retry => store.retry_message(),
                                Action::Receipt => store.check_receipt(),
                                Action::Refill => { store.snapshot_pending = true; let _ = store.requests().unbounded_send(BridgeRequest::Refill(store.session_id.clone())); },
                                Action::Attach { path } => { store.attachment_pending = true; let _ = store.requests().unbounded_send(BridgeRequest::AttachFile { seed: store.session_id.clone(), path }); },
                                Action::Driver { release, stale_holder } => { let _ = store.requests().unbounded_send(BridgeRequest::Driver { seed: store.session_id.clone(), release, stale_holder }); },
                                Action::LongSession { entries } => {
                                    let template = store.entries.iter().find(|entry| matches!(entry, qaqh_ringing_contract::state::TimelineEntry::Input { .. })).cloned().expect("real input fixture");
                                    for index in 0..entries {
                                        let mut entry = template.clone();
                                        if let qaqh_ringing_contract::state::TimelineEntry::Input { block_id, .. } = &mut entry { *block_id = qaqh_ringing_contract::fact::BlockId::new(format!("load-{index}")); }
                                        store.entries.push(entry);
                                    }
                                    store.timeline_list.reset(store.entries.len() + 1);
                                    store.timeline_list.scroll_to_end();
                                }
                                _ => unreachable!(),
                            }
                            cx.notify();
                        });
                        }
                    }
                }
                cx.notify();
            }
            Ok(_) => {}
            Err(err) => error = Some(err.to_string()),
        }
    }
    let current = daemon.current.as_ref().map(|store| {
        let store = store.read(cx);
        json!({"id": store.session_id, "draft":store.composer.text,"entries":store.entries.len(),
            "snapshot_pending":store.snapshot_pending,"stream_open":matches!(store.stream, StreamState::Open),
            "outbox":store.outbox,"driver":store.driver,"client_session_id":store.client_session_id,
            "images":store.images.len(),"attachments":store.attachments.len(),"attachment_pending":store.attachment_pending,
            "error":store.operation_error})
    });
    let state = json!({"sequence":SEQUENCE.load(Ordering::Relaxed),"error":error,"current":current,"last_line":daemon.lines.last(),
        "connected":matches!(daemon.connection, ConnectionState::Ready { .. }),"sessions":daemon.sessions.len(),
        "workspaces":daemon.workspaces.len(),"cached_sessions":daemon.stores.len()});
    let temporary = root.join("observed.tmp");
    std::fs::write(&temporary, serde_json::to_vec(&state).expect("state JSON"))
        .expect("write acceptance state");
    std::fs::rename(temporary, root.join("observed.json")).expect("publish acceptance state");
}
