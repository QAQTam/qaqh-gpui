//! 桌面系统通知与会话状态变迁提醒。

use gpui_kit::App;
use qaqh_gpui_session::store::SessionRow;

pub fn notify(cx: &App, session: &str, title: &str) {
    cx.show_system_notification(gpui_kit::SystemNotification {
        tag: format!("qaqh-{session}").into(),
        title: title.to_string().into(),
        body: "打开 QAQHarness 查看会话".into(),
        actions: Vec::new(),
    });
}

/// 名册前后对比:只在「进入待响应」「从运行回到空闲」两种真变迁时弹通知。
pub fn roster_notifications(old: &[SessionRow], rows: &[SessionRow], cx: &App) {
    for row in rows {
        if let Some(previous) = old.iter().find(|old| old.id == row.id) {
            let message = match row.activity {
                qaqh_core_vocab::ActivityState::WaitingUser
                    if previous.activity != qaqh_core_vocab::ActivityState::WaitingUser =>
                {
                    Some("会话正在等待你的响应")
                }
                qaqh_core_vocab::ActivityState::Idle
                    if matches!(
                        previous.activity,
                        qaqh_core_vocab::ActivityState::Running
                            | qaqh_core_vocab::ActivityState::Cancelling
                    ) =>
                {
                    Some("会话已结束本轮工作")
                }
                _ => None,
            };
            if let Some(message) = message {
                notify(cx, row.id.as_str(), message);
            }
        }
    }
}
