//! qaqh-gpui-session — 会话特性 crate(zed 模式:状态 + Render 同 crate):
//! SessionStore 及其时间线渲染(timeline)、reducer、markdown/code_panel/goal、
//! 会话侧控制条(controls)、内容替换(content)与草稿持久化(persist)。
//! orphan rule:impl Render for SessionStore 必须与类型同 crate,视图随状态走。
//! DaemonStore(shell)是顶层消息泵,向下调用本 crate;本 crate 绝不反向引用 shell。

pub mod code_panel;
pub mod content;
pub mod controls;
pub mod goal;
pub mod markdown;
pub mod persist;
pub mod reducer;
pub mod store;
pub mod timeline;