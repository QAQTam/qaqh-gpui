# QAQHarness GPUI

QAQHarness 的 GPUI 桌面前端，通过 RingingV2 连接 QAQHarness daemon。

支持 Linux x86_64 / Wayland。业务状态直接使用共享契约，连接、租约和订阅由 `qaqh-client` 管理。

## 安装候选包

解压发布包后运行：

```sh
bash install.sh                     # 默认 ~/.local，无需管理员权限
# 或 bash install.sh /absolute/prefix
```

安装包包含 GPUI 和 daemon。首次打开时会自动发现或启动 daemon；在「设置 → 服务商与模型」添加服务商、模型和密钥，再新建会话。「设置 → 账号」经系统浏览器登录 ChatGPT 套餐（SIWC）与 DeepSeek 账号，凭据保存在本机管理器、可查看余额。
运行需要 Wayland、Vulkan 驱动，以及 fontconfig、freetype、xkbcommon、ALSA、EGL 等系统库。安装器会校验二进制摘要和缺失的动态库。
卸载使用包内 `bash uninstall.sh`，会话、配置和草稿会保留。

## 日常使用

- `Ctrl+N` 新建会话，`Ctrl+K` 搜索会话，`Ctrl+F` 搜索消息，`Ctrl+,` 打开设置。
- Enter 发送，Shift+Enter 换行；支持中文 IME、复制/剪切/粘贴。
- 图片与 UTF-8 文本附件最大 4 MiB；图片可在输入区和历史中预览。
- 发送失败保留草稿与原命令身份；结果待确认时可查回执或重试原请求。受理与执行成功分别显示。
- 工作区管理、会话归档、控制权释放/接管、引用内容读取均使用 RingingV2 原生命令和 SDK。
- 草稿、最近会话、窗口大小、侧栏宽度、折叠状态和本地源码目录映射保存在 `$XDG_CONFIG_HOME/qaqh-gpui/workbench.json`。
- 只订阅当前会话，最多缓存 4 个会话视图；后台会话通过名册更新提示完成或等待响应。

```sh
qaqh-gpui --version
qaqh-gpui --probe
qaqh-gpui --diagnostics > diagnostics.txt
```

诊断仅包含版本、协议代际、epoch、水位及会话/命令身份，不导出正文、密钥或配置。日志上限约 1 MiB，保留一份轮转文件。

## 构建

使用 Python 3.11+ 和仓库指定的 Rust 工具链。在一个新的父目录检出本仓，然后准备固定版本的后端依赖：

```sh
git clone https://github.com/QAQTam/qaqh-gpui.git
cd qaqh-gpui
python3 scripts/prepare-sources.py
scripts/mem-guard.sh 1600 cargo build --locked -j1
```

UI 框架与组件库走 crates.io 精确 pin（`gpui-pre` / `gpui-pre-platform` / `gpui-component`，取舍见 `Cargo.toml` 注释），无需本地 vendor 检出；同级仓库只有 `qaqh-backend` 与 `mutil-ai`，由 `packaging/dependencies.json` 钉版。
准备脚本不会覆盖已有的同级仓库。已有开发检出时，后端须包含 `packaging/backend-client.patch` 中的原生改动：归档查询参数、共享 Todo 读模型和 daemon 子进程回收。这是后端源码修正，运行时直接使用新契约。

## 发布与验收

```sh
python3 scripts/check-capabilities.py
bash scripts/build-release.sh
```

产物位于 `dist/`：安装包、SHA-256、二进制/源码摘要与准确的源码快照。构建始终使用单任务；发布配置采用 16 个 codegen 单元和 thin LTO。
CI 对全部命令的能力分类、契约代际、构建、关键状态语义和五项原生验收设门禁，验收成功后才上传候选包。

本机运行同一个原生验收（仅连接临时目录里的 daemon 和回环模型，不消耗模型额度）：

```sh
cargo build --release --locked --features acceptance -j1
python3 scripts/acceptance.py --client target/release/qaqh-gpui --daemon target/release/qaqh-daemon
```

验收功能不进入默认发布构建，打包脚本拒绝带有该功能的二进制。
支持边界和验收记录见 [主力客户端交付清单](docs/PRIMARY-CLIENT.md)，命令逐项分类见 [capabilities.json](docs/capabilities.json)。

## 许可证

本项目自有代码可按 [MIT](LICENSE-MIT) 或 [Apache-2.0](LICENSE-APACHE) 使用。
第三方高亮查询沿用 [nvim-treesitter 的 Apache-2.0 许可证](assets/queries/README.md)。
