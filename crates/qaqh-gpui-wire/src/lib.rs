//! qaqh-gpui-wire — RingingV2 客户端接线层:桥线程(bridge)、投递状态机(delivery)、
//! 白名单诊断(diagnostics)、ASK v1 解析(ask)。无 gpui 依赖,可头less check/test。
//! 协议类型直用 qaqh-ringing-contract / qaqh-client,零转换层。

pub mod ask;
pub mod bridge;
pub mod delivery;
pub mod diagnostics;