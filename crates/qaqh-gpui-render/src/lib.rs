//! qaqh-gpui-render — 渲染原语:tree-sitter AST 语法高亮(highlight,query 资产
//! 在仓库根 assets/queries,include_str 三层上溯)与附件图像元素(media)。
//! 依赖 kit 色板,不依赖会话状态。

pub mod highlight;
pub mod media;