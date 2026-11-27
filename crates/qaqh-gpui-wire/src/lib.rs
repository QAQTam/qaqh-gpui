//! qaqh-gpui-wire — RingingV3 客户端接线层:桥线程(bridge)、投递状态机(delivery)、
//! 白名单诊断(diagnostics)。无 gpui 依赖,可头less check/test。
//! 协议类型直用 `qaqh-ringing-v3` / `qaqh-core-vocab`,零转换层。

pub mod bridge;
pub mod delivery;
pub mod diagnostics;
