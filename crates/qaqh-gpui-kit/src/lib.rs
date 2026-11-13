//! qaqh-gpui-kit — 外观/设置/组件原语层:settings(本地偏好 + 原子持久化)、
//! theme(语义色板 × 亮/暗 + 组件库主题映射)、kit(手绘控件原语)。
//! settings 与 theme 按设计双向耦合(settings::update 唯一写入口内嵌主题副作用),
//! 同 crate 安置使环自然消解,不引入回调注入机制。
//! 分层设计借鉴 zed 的 Appearance/settings(仅借鉴设计,不搬代码)。

pub mod kit;
pub mod settings;
pub mod theme;