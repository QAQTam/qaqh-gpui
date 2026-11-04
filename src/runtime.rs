//! tokio ↔ gpui 桥。
//!
//! `qaqh-client` 是 RingingV2 传输权威(TUI 同款),它活在 tokio 上;
//! GPUI 活在自己的 executor 上。桥 = 一个独立 tokio 运行时线程 +
//! unbounded channel 把状态行投回 GPUI 前台泵。协议类型原样使用,
//! 不做任何转换层(宪法 #9)。

use std::time::Duration;

use qaqh_client::Client;
use qaqh_client::ClientOptions;
use qaqh_client::discovery::DiscoveryExt;
use qaqh_client::discovery::ensure_daemon_running;
use qaqh_client::discovery::read_discovery;
use qaqh_client::projection::sanitize_session_list;
use serde_json::json;

/// `--probe`:无 GUI 连通性验证,打印与窗口相同的状态行。
pub fn probe() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        run_connection(|line| println!("{line}")).await;
    });
}

/// 连接序列:discovery → connect → open_v2 → session.list。
/// 每一步经 `report` 上报;失败即如实上报并停止,不伪造成功。
pub async fn run_connection(report: impl Fn(String)) {
    report("qaqh-gpui:查找 daemon discovery…".into());

    let discovery = match read_discovery() {
        Ok(discovery) => {
            let url = match discovery.base_url() {
                Ok(url) => url,
                Err(error) => error.to_string(),
            };
            report(format!("discovery 就绪:{url}"));
            discovery
        }
        Err(error) => {
            report(format!("本地 discovery 不可读({error}),尝试拉起 daemon…"));
            match ensure_daemon_running(Duration::from_secs(6)) {
                Ok(discovery) => {
                    report(format!("daemon 已就绪:{}", discovery.base_url().unwrap_or_default()));
                    discovery
                }
                Err(error) => {
                    report(format!("daemon 不可达:{error}"));
                    return;
                }
            }
        }
    };

    let _ = discovery;
    let client = match Client::connect(ClientOptions {
        launch_daemon_if_missing: false,
        ..ClientOptions::default()
    }) {
        Ok(client) => client,
        Err(error) => {
            report(format!("连接失败:{error}"));
            return;
        }
    };
    report("client 已建立".into());

    let open = match client.open_v2().await {
        Ok(open) => open,
        Err(error) => {
            report(format!("v2 open 失败:{error}"));
            return;
        }
    };
    report(format!(
        "v2 open:client_session={},epoch={}",
        open.client_session_id, open.server_epoch
    ));

    match client
        .service_v2::<_, serde_json::Value>("session.list", json!({}))
        .await
    {
        Ok(value) => {
            let entries = sanitize_session_list(value);
            let count = entries.as_array().map(Vec::len).unwrap_or(0);
            report(format!("session.list:{count} 个会话"));
            for entry in entries.as_array().unwrap_or(&Vec::new()).iter().take(8) {
                report(format!(
                    "  · {} 「{}」",
                    entry["session_id"].as_str().unwrap_or("?"),
                    entry["title"].as_str().unwrap_or("(无标题)"),
                ));
            }
        }
        Err(error) => report(format!("session.list 失败:{error}")),
    }
}
