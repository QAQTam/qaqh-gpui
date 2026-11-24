//! 桌面系统通知与会话状态变迁提醒。

use gpui_kit::App;
use qaqh_client::SessionListEntry;
use qaqh_gpui_session::store::SessionRow;

pub fn notify(cx: &App, session: &str, title: &str) {
    cx.show_system_notification(gpui_kit::SystemNotification {
        tag: format!("qaqh-{session}").into(),
        title: title.to_string().into(),
        body: "打开 QAQHarness 查看会话".into(),
        actions: Vec::new(),
    });
}

pub fn roster_notifications(old: &[SessionRow], entries: &[SessionListEntry], cx: &App) {
    for entry in entries {
        if let Some(previous) = old.iter().find(|row| row.id == entry.meta.session_id) {
            let message = match entry.status {
                qaqh_client::SessionRunStatus::WaitingUser
                    if previous.status != qaqh_client::SessionRunStatus::WaitingUser =>
                {
                    Some("会话正在等待你的响应")
                }
                qaqh_client::SessionRunStatus::Idle
                    if previous.status == qaqh_client::SessionRunStatus::Running
                        || previous.status == qaqh_client::SessionRunStatus::Cancelling =>
                {
                    Some("会话已结束本轮工作")
                }
                _ => None,
            };
            if let Some(message) = message {
                notify(cx, &entry.meta.session_id, message);
            }
        }
    }
}
