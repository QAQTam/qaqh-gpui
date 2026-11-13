//! UI delivery state; payloads and receipts remain canonical Ringing types.
use qaqh_client::ringing_v2::{
    RingingV2CommandAck, RingingV2CommandStatus, RingingV2ExistingResult,
};
use qaqh_client::{ConversationSendMessage, RingingCommandAckStatus, RingingCommandState};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
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
            Self::Uncertain => "结果待确认，可查回执或重试原请求",
        }
    }
    pub fn from_receipt(state: RingingCommandState) -> Self {
        match state {
            RingingCommandState::Accepted | RingingCommandState::Running => Self::Accepted,
            RingingCommandState::Succeeded => Self::Succeeded,
            RingingCommandState::Rejected => Self::Rejected,
            RingingCommandState::Failed => Self::Failed,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outbox {
    pub command_id: String,
    pub message: ConversationSendMessage,
    pub state: DeliveryState,
    pub detail: String,
}

impl Outbox {
    pub fn acknowledge(&mut self, ack: &RingingV2CommandAck) {
        self.state = match &ack.existing {
            Some(RingingV2ExistingResult::CommandReceipt { state, .. }) => {
                DeliveryState::from_receipt(*state)
            }
            _ if ack.status == RingingCommandAckStatus::Accepted => DeliveryState::Accepted,
            _ => DeliveryState::Rejected,
        };
        self.detail = ack
            .message
            .clone()
            .or_else(|| ack.code.clone())
            .unwrap_or_default();
    }
    pub fn receipt(&mut self, status: &RingingV2CommandStatus) {
        self.state = DeliveryState::from_receipt(status.state);
        self.detail = status.error_code.clone().unwrap_or_default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepted_is_not_completed() {
        assert_eq!(
            DeliveryState::from_receipt(RingingCommandState::Running),
            DeliveryState::Accepted
        );
        assert_eq!(
            DeliveryState::from_receipt(RingingCommandState::Failed),
            DeliveryState::Failed
        );
        assert_eq!(
            DeliveryState::from_receipt(RingingCommandState::Succeeded),
            DeliveryState::Succeeded
        );
    }
}
