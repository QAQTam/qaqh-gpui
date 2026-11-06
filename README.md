# QAQHarness GPUI

QAQHarness 的 GPUI 桌面前端，通过 RingingV2 连接 QAQHarness daemon。

## 构建

从同一个父目录检出这些仓库。将 `gpui-vendor` 检出到 `gpui`，以匹配 Cargo 的相对路径依赖。

```sh
git clone https://github.com/QAQTam/gpui-vendor.git gpui
git -C gpui checkout d1520011e6204b95b320ee40a5a6b11879082426
git clone https://github.com/QAQTam/mutil-ai.git
git clone https://github.com/QAQTam/qaqh-backend.git
git clone https://github.com/QAQTam/qaqh-gpui.git
cd qaqh-gpui
cargo build --locked
```

## 许可证

本项目自有代码可按 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 使用。
第三方高亮查询沿用 [nvim-treesitter 的 Apache-2.0 许可证](assets/queries/README.md)。
