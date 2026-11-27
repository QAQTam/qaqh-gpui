//! UI 投递状态机；载荷与回执一律是 RingingV3 强类型。
//!
//! V3 没有"异步回执查询"：`CommandResponse::Accepted` 只表示**已入队**，真正的
//! 结果由会话事件流宣告（`TurnStarted` / `TurnFinished` / `ToolCallUpdated`）。
//! 所以这里刻意保留 `Uncertain`——"已发送但还没有任何事件证明"。
use qaqh_ringing_v3::CommandResponse;
use serde::{Deserialize, Serialize};

use crate::bridge::DeliveryResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeliveryState {
    Sending,
    Accepted,
    Succeeded,
    Rejected,
    Failed,
    Uncertain,
}

impl DeliveryState {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Sending => "正在发送",
            Self::Accepted => "已受理，等待执行结果",
            Self::Succeeded => "执行成功",
            Self::Rejected => "请求被拒绝",
            Self::Failed => "执行失败",
            Self::Uncertain => "结果待确认，可查事件流或重试原请求",
        }
    }

    /// 命令域回执 → 投递状态（受理 ≠ 完成）。
    pub fn from_response(response: &CommandResponse) -> Self {
        match response {
            CommandResponse::Accepted { .. }
            | CommandResponse::CancelAccepted
            | CommandResponse::InteractionAccepted => Self::Accepted,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outbox {
    /// 幂等键：断线重放复用同一个 id，服务端返回原回执。
    pub command_id: String,
    pub state: DeliveryState,
    pub detail: String,
}

impl Outbox {
    pub fn new(command_id: impl Into<String>) -> Self {
        Self {
            command_id: command_id.into(),
            state: DeliveryState::Sending,
            detail: String::new(),
        }
    }

    pub fn apply(&mut self, result: &DeliveryResult) {
        match result {
            DeliveryResult::Accepted { .. } => {
                self.state = DeliveryState::Accepted;
                self.detail.clear();
            }
            DeliveryResult::Rejected(detail) => {
                self.state = DeliveryState::Rejected;
                self.detail = detail.clone();
            }
            DeliveryResult::Unknown(detail) => {
                self.state = DeliveryState::Uncertain;
                self.detail = detail.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qaqh_core_vocab::InputId;

    #[test]
    fn accepted_is_not_completed() {
        let response = CommandResponse::Accepted {
            input_id: InputId::new("inp-1").expect("id"),
        };
        assert_eq!(DeliveryState::from_response(&response), DeliveryState::Accepted);
    }

    #[test]
    fn outbox_distinguishes_rejected_from_uncertain() {
        let mut outbox = Outbox::new("cmd-1");
        outbox.apply(&DeliveryResult::Rejected("被策略拒绝".into()));
        assert_eq!(outbox.state, DeliveryState::Rejected);
        outbox.apply(&DeliveryResult::Unknown("断线".into()));
        assert_eq!(outbox.state, DeliveryState::Uncertain);
    }
}
