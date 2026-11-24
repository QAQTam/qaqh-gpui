//! qaqh-gpui-shell — 工作台壳特性 crate:DaemonStore(顶层消息泵)及其根渲染
//! (sidebar)、设置编辑(settings_ui)、工作台视图(views)、桌面壳侧(desktop:
//! 连接/欢迎/系统通知/关于)。orphan rule:impl Render for DaemonStore 随类型走。
//! 向下调用 session/wire/kit;session 绝不反向引用本 crate。

pub mod daemon;
pub mod desktop;
pub mod pages;
pub mod settings_ui;
pub mod shell_panel;
pub mod sidebar;
pub mod titlebar;
pub mod views;
