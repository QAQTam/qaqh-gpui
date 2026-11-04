# qaqh-gpui 设计探索报告

日期:2026-11-05 深夜(环境与设计探索阶段交付)。
状态:骨架已提交(`7c2b6fd`),`cargo check -j1` 在内存守卫下后台验证中。

## 0. 结论摘要

1. **连接方案已定**:GPUI 前端直接 path 依赖后端仓的 `qaqh-client`(RingingV2 传输权威实现,与 `qaqh-tui` 同路)。前端不自建传输、不写协议转换层(宪法 #9)。
2. **RingingV2 是 HTTP + SSE**,不是 WS/stdio。命令面为 `POST /ringing/v2/commands` + 类型化 envelope;读模型为 bootstrap 快照 + 单条 SSE 事件流,`epoch:seq` 水位同步。
3. **gpui 引入方式**:本地检出 `../gpui`(zed 拆平台版,rev `d1520011e6204b95b320ee40a5a6b11879082426`)以 path 依赖引入,`[patch.crates-io]` 五条必须随行复制(跨 workspace 不传播)。
4. **最大工程约束是内存**(可用 ~1.5Gi + swap 几乎满):构建用 `-j1` + `scripts/mem-guard.sh`;依赖树 729 包,`check` 先行,`build` 只在 `check` 绿后做。

## 1. 环境探查

| 项 | 值 | 影响 |
|---|---|---|
| CPU | 8 核 | -j1 下利用率极低,但内存优先 |
| 内存 | 7.7Gi,可用 ~1.3–1.6Gi | **瓶颈**。两个 ZCode 实例常驻(~340MB×2 + 两个 zcode-cli ~220MB×2) |
| swap | 3.8Gi,已用 3.5Gi | 系统已深处换页,编译峰值控制必须保守 |
| 磁盘 | 79Gi 可用 | 充裕;`target/` 预计 +4–6Gi |
| 工具链 | 1.98.1 已安装(与 `../gpui/rust-toolchain.toml` 对齐) | 零下载 |
| 前例 | `../gpui/target` 有 debug 产物 → 本机构建过 gpui | 系统级依赖(fontconfig/xkbcommon/wayland dev)已在 |

**资源策略**(用户睡前指令:内存友好,避开两个后台 ZCode 的内存竞争):
- `scripts/mem-guard.sh [阈值MB] <cmd>`:可用内存 < 阈值时每 60s 复查,回升才放行。
- 编译一律 `-j1`;先 `check`(无 codegen,峰值低)后 `build`。
- `[profile.dev] debug = "limited"` 降低产物体积。

## 2. 依赖引入(已落地)

```toml
gpui           = { path = "../gpui/crates/gpui", default-features = false,
                   features = ["font-kit", "wayland", "x11"] }
gpui_platform  = { path = "../gpui/crates/gpui_platform", features = ["wayland", "x11"] }
qaqh-client    = { path = "../qaqh-backend/crates/qaqh-client" }
```

- **(rev) 的处理**:`../gpui` 是无远端本地仓,git rev 钉版不可用;以 path 依赖 + Cargo.toml 注释记录 rev `d1520011e6204b95b320ee40a5a6b11879082426` 达成同等可追溯性。gpui 版本 0.2.2,edition 2024。
- **拆平台版结构**:`gpui` crate 平台无关;Linux 后端经 `gpui_platform`(feature `wayland`/`x11` → `gpui_linux`)接线;应用入口是 `gpui_platform::application() -> gpui::Application`(hello_world 示例同款)。
- **[patch.crates-io] 五条逐字复制自 gpui workspace**(windows-capture / khronos-egl / calloop / async-task / async-process,均为 zed fork)。cargo 的 patch 只在定义它的 workspace 生效,外部消费缺了就编上游版本,calloop/async-process 会直接失败。
- **跨仓 path 依赖先例**:`qaqh-tui-app` 已用完全相同的姿势依赖 `qaqh-client`,其 Cargo.toml 注释记录了取舍:跨 workspace 继承(`workspace = true`)正常解析,代价是后端仓必须与相对路径共存。
- 依赖树规模:**729 包**(cargo metadata 实测),含 wgpu(git rev)、resvg、image、icu_segmenter 等重物。

## 3. 后端协议面(RingingV2 权威事实)

全部事实取自源码与 `docs/`(report-02、contract-decisions、spec-exec-session-lifecycle):

### 3.1 传输与端点

| 端点 | 方法/形态 | 用途 |
|---|---|---|
| `/ringing/v2/clients/open` | POST | 开客户端会话,得 `client_session_id` + `server_epoch` + lease 参数 + **双契约代际** |
| `/ringing/v2/commands` | POST | 提交命令 envelope;`X-QAQH-Client-Session-Id` 头必带(lease 判定只看头) |
| `/ringing/v2/commands/{id}` | GET | 命令 receipt(幂等重放,含 `sync_result`) |
| `/ringing/v2/sessions/{id}/bootstrap` | GET | 会话快照 `SessionState`(含 `seq` 水位) |
| `/ringing/v2/sessions/{id}/events` | GET SSE | 单条事件流,`?since_cursor=epoch:seq` |
| `/ringing/v2/service/{method}` | POST | 类型化 service 面(如 `session.list`,参数 `{}` → `SessionListEntry[]`) |
| `/ringing/v2/content/{id}` | GET | 内容寻址下载(工具输出大件) |
| `/ringing/v2/leases/renew` | POST | lease 续期(SDK `run_renewal` 自动做,前端不碰) |
| `/ringing/v2/pairing/tokens`、`/ringing/v2/devices*` | POST/GET | 设备配对/管理 |
| `/control/v1/stop(-if-idle)` | POST | daemon 生命周期 |

### 3.2 命令面(最高准则,宪法 #9)

- envelope:`RingingV2CommandEnvelope { schema: "qaqh.Ringing", version: 2, contract_revision, command_id(ULID), client_instance_id, client_session_id, target, preconditions, command }`,客户端侧 `validate()` 先行。
- 当前代际:**`ringing-commands-7`(命令)/ `ringing-session-2`(读模型+事件)**,open 响应携带,客户端严格校验。
- 发送入口:`client.send_command_v2_typed(target: CommandTarget, command: RingingCommand, options: CommandOptions) -> RingingV2CommandAck`。ULID 自动生成;同 `command_id` 重发得幂等 receipt(D2,含 `sync_result`)。
- `CommandTarget`:Global / Session / Workspace / ExecSession——target 即路由,前端按交互语境选。

`RingingCommand` 词表(源码实测,serde tag = 命令名字符串):

| 域 | 命令 |
|---|---|
| 会话 | `session.create { cwd,title,profile,effort,model_selection,tool_mode,custom_tools }` / `session.settings.patch` / `session.resume` / `session.attach` / `session.close` / `session.rename` / `session.archive` / `session.unarchive` / `session.delete` / `session.shutdown` |
| 对话 | `conversation.send_message { text, images, attachments, message_id, input_purpose }` / `conversation.cancel { turn_id? }` |
| 交互 | `interaction.ask.respond { interaction_id, answers }` / `interaction.ask.dismiss` / `plan.review.respond { approved, message?, autonomous }` / `tool.permission.respond { tool_call_id, approved, trust_folder }` |
| 席位 | `driver.claim { client_session_id, stale_holder? }` / `driver.release` |
| 执行会话 | `exec_session.interrupt` / `exec_session.terminate`(target 必为 ExecSession) |
| 模型/配置 | `provider.list/upsert/delete`、`provider.model.upsert/delete`、`model.selection.set_default`、`config.save`、`config.set_permission_level`、`profile.create/set_default/apply/save_current/delete`、`agent.reload_config` |
| 工作区 | `workspace.set/create/rename/delete/move_session/detach` |
| 子代理 | `subagent.spawn { tools, model, base_url, max_tokens, workspace, … }` |

### 3.3 读模型同步(水位 + resync)

- `seq` = epoch 内 0 基单调序号;bootstrap `state.seq` 是**互斥上界**(已覆盖 `seq < state.seq`)。客户端:跳 `<`,处理 `>=`(D1)。
- SSE 游标 `epoch:seq`;epoch 不匹配或 `since < P` → daemon 发 `ringing.resync_required` → **清空暂态、重新 bootstrap**(D6:明确允许丢暂态,未 seal 的流式文本不承诺恢复)。
- 事件帧按 id 幂等吸收:timeline 同 `block_id` 合并、tool/interaction/turn 按 id 替换。
- SSE keepalive 是注释行字节:SDK 已按**字节层探活**处理(`ClientV2SubscriptionEvent::KeepAlive`),前端泵不得把 KeepAlive 当事件。

### 3.4 事件词表(`qaqh_ringing_contract::events::SessionEvent`,当前 20 变体)

会话骨架:`ActivityChanged` / `TurnStarted` / `TurnPhaseChanged` / `TurnEnded` / `SessionMetaChanged` / `DriverChanged`
模型输出:`MessageDelta { block_kind: reasoning|answer, text 累计替换 }` / `TimelineAppended`
工具线:`ToolArgumentsDelta { tool_name, text 累计替换 }` / `ToolDeclared` / `ToolStateChanged` / `ToolOutputDelta { stdout|stderr 尾窗 }` / `ToolFinished { outcome, output, code_delta, effects }`
执行会话线:`ExecSessionStarted/Changed/OutputDelta { offset 追加去重 }/Finished`
交互:`InteractionRequested { InteractionView }` / `InteractionResolved`
代理:`SubagentUpdated { SubagentView }`

注意:docs 里 report-02 说 15 种、D4 说 +2——源码已到 20,ExecSession* 事件已落地(spec-exec-session-lifecycle 已实施进契约)。前端按源码为准。

## 4. 前后端连接方案

### 4.1 三层结构,零转换层

```text
┌─ GPUI 前台(主线程)──────────────────────────────┐
│ ui/(元素构建) ← store/(GPUI 实体,直持 contract 类型) │
│        ↑ Entity::update + cx.notify                 │
│   pump(cx.spawn:futures mpsc → store reducer)       │
└──────────────┬──────────────────────────────────────┘
               │ futures::channel::mpsc(跨线程 waker,GPUI executor 驱动)
┌──────────────┴──────────────────────────────────────┐
│ runtime/:独立线程 + 私有 tokio 运行时(2 worker)      │
│   qaqh-client::Client(open/subscribe/send/renew)    │
└──────────────┬──────────────────────────────────────┘
               │ HTTP + SSE(axum daemon,discovery 文件定位)
        qaqh-backend daemon
```

要点:
1. **传输唯一权威 = qaqh-client**。连接生命周期、lease 续期(`run_renewal`)、SSE 解码、字节层探活、幂等重试全在 SDK;TUI 注释明言"本仓不再自建",GPUI 侧同样不再自建。
2. **类型直用**:store 实体字段直接持有 `qaqh_ringing_contract` 类型(`ToolCallView`/`InteractionView`/`TimelineEntry`/`SessionEventFrame`…)。不存在"前端 DTO"——为 ringging 写一个转换层属于宪法 #9 点名的懒政。
3. **tokio ↔ GPUI 桥是必要的执行器对接,不是协议转换**:reqwest future 必须在 tokio context 上 await;GPUI future 必须在其前台 executor 上驱动。桥体 = 一个 tokio 线程 + unbounded channel;事件经泵(`cx.spawn` 循环)进 store。
4. **命令是异步请求**:UI 事件 → `store` 调 `runtime` → tokio 上 `send_command_v2_typed` → ack 经同一 channel 回流更新 UI 状态。receipt 幂等交给协议,UI 不造重试。

### 4.2 会话订阅生命周期(SDK 托管流,前端只管回调)

核实 `qaqh-client/src/v2_stream.rs` 后确认:**bootstrap / 游标推进 / 断线退避重连 / epoch 失效归零 / resync 后重订阅全部由 SDK 的 `V2Stream` 托管**,前端不自建订阅循环:

```text
Client::connect(ClientOptions { handlers, … })      // 回调在 connect 前挂好
  → open_v2(校验双契约代际,不匹配即拒绝连接)
  → attach(session_id)            // = session.resume 命令;lease 重协商后 SDK 自动重放
  → activate_stream(session_id)   // SDK 起一条 per-seed V2Stream:
                                  //   无游标→bootstrap_v2 取 state.seq 作游标→subscribe
                                  //   事件逐帧 on_v2_event(seed, SessionEventFrame),游标随 seq 推进
                                  //   resync_required→游标归零→SDK 自动重新 bootstrap+重订阅
                                  //   断线→退避重连;epoch 变化→游标失效归零
```

前端回调(SDK tokio 线程上投递,TUI 已验证同款姿势):

| 回调 | 前端动作 |
|---|---|
| `on_v2_event(seed, frame)` | 按 id 幂等进 reducer(D1 残余重叠吸收),更新 `SessionStore` |
| `on_v2_reset(seed, reset)` | 清该会话暂态缓冲(未 seal 文本/参数),自行 `bootstrap_v2` 重灌快照(D6);SDK 侧已自行重订阅 |
| `on_v2_status(seed, V2StreamStatus)` | 连接状态徽标(Connecting/Open/Reconnecting/Closed) |
| `on_liveness()` | 字节层存活信号(keepalive 也算),驱动「失联」判定,禁止按时间猜 |

多会话 = 每 seed 一条 SDK 流(按 seed 键控,互不干扰)+ 一个 `SessionStore` 实体;`session.list` service 做侧栏。前台只显示中的会话保持 activate,离开的会话 `deactivate_stream`。

### 4.3 GPUI 侧设计

目录(烟雾版之后的演进目标):

```text
src/
  main.rs            — --probe 分流 + app::launch
  runtime/           — 唯一碰 tokio 的模块
    mod.rs bridge.rs   (tokio 线程 + channel;run_connection/probe)
    pump.rs            (每会话订阅泵,Event→Action 流)
  store/             — GPUI 实体(状态唯一归口)
    daemon.rs          (discovery/client 状态、会话列表)
    session.rs         (单会话:turn/timeline/tools/interactions/subagents)
    interaction.rs     (待应答交互队列:权限/ask/plan)
  protocol/          — reducer:SessionEvent → store 变更(纯函数,直用 contract 类型)
  ui/                — 元素构建(theme / sidebar / timeline / tool_card / composer / permission_sheet)
```

GPUI 惯用法(已从 0.2.2 源码核实):
- `gpui_platform::application().run(|cx: &mut App| …)`;`cx.open_window(opts, |_, cx| cx.new(…))`。
- `Entity<T: Render>` 本身是 `IntoElement`——子实体渲染自动注册依赖,父级随动重绘,无需手写 observer。
- 后台改状态:`entity.update(cx, |state, cx| { …; cx.notify() })`,`cx.spawn(async move |cx: &mut AsyncApp| …)` 泵。
- 无全局 GC 压力,实体树即 UI 树;timeline 长列表后续用 `list()` 均匀列表虚拟化。

## 5. 里程碑

| 里程碑 | 内容 | 验收 |
|---|---|---|
| M0(本次) | 骨架 + 依赖 + 烟雾窗口 + `--probe` | `cargo check -j1` 绿;probe 如实报告连接结果 |
| M1 只读 | session.list 侧栏 + bootstrap + 单会话 SSE 泵 + timeline 渲染(文本块/工具卡片终态) | 断线→resync 自动恢复;水位跳帧正确 |
| M2 交互 | composer(发消息命令)、权限/ask/plan 应答、中断 | 三种 interaction 全链路;幂等重发不重复执行 |
| M3 多会话 | 多泵并行、subagent 视图、exec 会话输出尾窗/偏移去重 | 子会话从 effects 关联(parent_call_id) |
| M4 打磨 | 主题、字体捆绑决策、快捷键、内容下载(大件)、打包 | — |

## 6. 风险与开放问题

1. **构建内存**:-j1 下重包(wgpu/resvg/icu)单包峰值仍可能 ~1Gi;守卫阈值 1050MB,必要时降到 950MB 再试。build 期(链接)比 check 更险,放到白天人多时也可。
2. **运行内存**:GPUI 渲染常驻估 200–400MB,与两个 ZCode 共存需实测;M0 起窗即记录。
3. **wgpu/驱动**:zed fork wgpu 在本机 GPU 的表现未经窗口实测;`--probe` 只验协议层。
4. **字体**:Linux 上 gpui 走系统字体;若默认字体族解析失败,参考 `examples/example_support/fonts.rs` 捆绑。
5. **契约代际**:`ringing-commands-7` / `ringing-session-2` 仍在演进(exec spec 落地中);前端锁 SDK 即锁契约,无漂移风险。
6. **qaqh-electron 功能面对照**:GPUI 版先覆盖 TUI 已验证的只读+交互路径,Electron 的设备配对/设置界面列 M3+ 再评。

## 7. 烟雾版使用

```bash
cargo run -- --probe     # 无 GUI:打印 discovery → open_v2 → session.list 全过程
cargo run                # 窗口:同序列状态行滚动显示
scripts/mem-guard.sh 1050 cargo build -j1   # 低水位时的构建姿势
```
