# 主力客户端交付交接 — 2026-11-07

用户因额度见底要求 handoff / 收工。本轮选择暂停交接，不能宣称最终发布包完成。所有修改尚未提交。

## 已实现

- 发送 Outbox：发送/受理/成功/拒绝/失败/待确认，持久化原命令 ID 和原始 canonical payload，失败保留草稿，查回执和同身份重试。
- 连接持续恢复、手动重连、旧连接回调代际隔离、快照等待期间缓冲事件和重新灌入。
- 安装/卸载、桌面图标、固定依赖、源码快照与摘要、首次启动引导、能力清单和 CI。
- 图片/UTF-8 附件与预览、引用内容读取、工作区管理、归档查询与搜索、driver 取得/释放/接管。
- 草稿/最近会话/窗口/侧栏/本地源码映射恢复、通知、快捷键、复制和消息搜索。
- 只订阅当前会话、最多缓存 4 个会话、输出尾窗与图片/高亮/Markdown 缓存预算。
- 白名单结构化诊断、版本 CLI；session.list / todo.list 直接使用权威读模型。

## 最新验证（无需重跑全量测试）

- `dist/unit-tests.log`：前端 10/10 通过，包含中文输入编辑和发送状态语义。
- `dist/sdk-identity-test.log`：SDK 现有定向身份测试 1/1 通过，57 个其余测试未运行。
- `dist/acceptance.json`：`passed: true`，`dist/native-acceptance.log` 为对应完整日志。
- 五项真实 GPUI/store/bridge/SDK/临时 daemon 验收通过：隔离安装首条消息、失败恢复与同命令重放、两客户端 driver 交接、自动拉起和强杀 daemon 后恢复、10002 条时间线。
- 额外已验证：图片/文本附件、归档查询、草稿/最近会话恢复、服务端成功但 ACK 未保存时跨应用重启取回原回执且不重复执行。
- 长时间线 RSS 156544 KiB（约 153 MiB）；验收控制轮询往返 2.003 秒，**不是实际键盘输入延迟测量**。
- 固定后端版本的 `packaging/backend-client.patch` 已在干净源码上做 `git apply --check`，通过。
- 远端 CI 尚未触发，不要声称已通过。

## 关键协议修复

后端同级仓 `/home/qaqtamsy/Projects/qaqh-backend` 中的本轮原生修改均包含在 `packaging/backend-client.patch`：

1. session.list 新增 include_archived；查询仍默认排除归档，GPUI 显式请求包含归档。
2. Todo DTO 从 runtime 移入 ringing-contract，前后端直接使用同一类型。
3. SDK 启动 daemon 后用等待线程回收退出的子进程，避免僵尸 PID 阻止再次启动。
4. ClientOptions 新增 client_instance_id；RingingSession 构造时确定逻辑身份并在续租时保持。构造器新增该参数，已更新所有已知调用点和 SDK 集成测试调用。
5. 删除未被其它源码调用的 open_v2_with_instance_id；open_v2 使用配置的身份并广播 adopt 状态。GPUI 直接读取 connect_async 已协商的 v2_session_state，避免重复 open 覆盖逻辑身份。
6. GPUI workbench 在连接前保存逻辑身份，保证重开应用后可以查询原命令回执。

曾遇到的 409 是验收脚本拿另一个逻辑客户端重放原命令造成的归属冲突，不是正常幂等响应。脚本已经改为原客户端重放，并严格要求 accepted 与 succeeded existing receipt。不要恢复兼容 409 的分支。

后端开始前已有其它用户改动：.cargo 配置、Cargo 清单/锁、agent-runtime/session_loop、registry/messaging、service.rs 和 session handler 的模型选择修改，以及未跟踪的 harmony-napi。**不要覆盖或整体提交这些既有工作。** 本轮在 service.rs/session handler 只改了归档查询相关行，打包补丁只包含本轮改动。

## 剩余工作：最终重打包与交付

生产源码已经通过上述开发配置原生集成验证。优化前端与 daemon 先前均成功构建，并生成过 38 MiB 包；但是那个包和当前 `target/release/qaqh-gpui` 在最后的 SDK 逻辑身份修复之前，**视为旧候选包，不要发布**。

最后一次最终构建因用户要求暂停已停止（工具会话 1560；其 cargo/rustc 已终止）。没有构建需要继续等待。缓存、已完成二进制和日志保留。不要依据文件存在认定最终修复已进入发布包。

恢复后执行：

```sh
bash scripts/build-release.sh > dist/final-release-build.log 2>&1
```

这会编译最终生产源码、复用 daemon 构建、重新生成安装包。依赖已全部热缓存；本机旧 CPU 下前端 thin LTO 重链接仍需约 10 分钟，使用 -j1，避免并发构建。

随后仅做必要收尾：

1. 确认构建 exit 0，`target/release/qaqh-gpui --version` 不含 `[acceptance]`，生产版拒绝 `--acceptance-root`。
2. 核对 `dist/qaqh-gpui-linux-x86_64/manifest.json` 内所有文件 SHA-256 和 `.tar.gz.sha256`。如有必要用临时 prefix 检查最终包安装/卸载；不要覆盖用户安装。
3. 将当前验收报告保留在 dist，更新 `docs/PRIMARY-CLIENT.md` 的最终状态和 `docs/HANDOFF.md` 顶部。不要因仅剩构建就直接勾成全部完成。
4. 文档改完后可运行 `python3 scripts/package.py` 以刷新包内准确源码快照；只改文档无需重新编译 Rust。
5. 按完整清单审计后再将 goal 标为 complete。未完成前保持暂停/进行中的真实状态。

主要文件入口：src/delivery.rs、workbench.rs、bridge.rs、store.rs、desktop.rs、sidebar.rs、content.rs、media.rs、diagnostics.rs；发布代码在 scripts/、packaging/、.github/workflows/release.yml。

本轮约束：Linux x86_64 / Wayland；搜索当前最近 256 条会话；能力分类为 35 supported / 10 deferred / 1 excluded，详见 docs/capabilities.json。验收驱动只有 `acceptance` feature 才编译，默认发布构建必须关闭，打包器会检查。
