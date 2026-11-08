# HANDOFF — qaqh-gpui 工作状态(防上下文压缩丢失)

> 用户因额度要求暂停。最新且完整的续接说明见 [PRIMARY-CLIENT-HANDOFF.md](PRIMARY-CLIENT-HANDOFF.md)：功能和原生验收已完成，最终身份修复后的优化重打包尚未完成，旧候选包不可发布。

> 2026-11-07：主力客户端交付正在收尾，以 [PRIMARY-CLIENT.md](PRIMARY-CLIENT.md) 的当前清单和验收记录为准。下文保留早期里程碑历史，不代表当前缺口。
>
> 新增模块：delivery（发送状态）、workbench（桌面状态持久化）、desktop/sidebar（桌面控制和侧栏）、content/media（引用与图片）、diagnostics（白名单诊断）。
> 后端原生修改：session.list 支持 include_archived；Todo 读模型移入 ringing-contract；SDK 回收自行启动的 daemon 子进程。打包源修正集中在 packaging/backend-client.patch。

> 更新:2026-11-06(第四轮)。M1-M10 + **M11 会话级换模型 + M12 分组管理** 全部落地。
> `cargo check` 0 错 / `cargo test` 5/5 / `cargo build` 链接成功;唯一 warning =
> windows-capture patch 未用(有意 mirror ../gpui workspace)。

## 第四轮新增(M11/M12)

### M11 会话级换模型(model_selection)
- 依赖新增:`qaqh-types = path`(ModelSelection/SettingValue wire 类型权威)
- bridge:`PatchSessionSettings` 增 `model: Option<(provider_id, model_id)>` +
  `revision: Option<u64>`;组 `SettingValue::Use { ModelSelection { options: default } }`;
  **乐观并发**:preconditions = SettingsRevision(本地持有上次 ack.revision);
  冲突(`code == "settings_revision_conflict"`)→ `BridgeMsg::SettingsRevision(seed, None)`
  失效对账,成功 → `SettingsRevision(seed, ack.revision)`
- store:SessionStore `model/model_options/settings_revision`;patch_effort/tools/model
  全部带 revision;DaemonStore 把 provider 目录投影(封顶 24)喂给所有会话
- UI:会话头「模型:默认/provider 的 model_id」chip,点击循环 model_options
- 类型事实:`SettingValue<T> = enum { Use{value:T}, Inherit }`(qaqh-types/provider.rs);
  provider 目录 = `{revision, providers: {<id>: {models: {<model_id>: ModelDef}}}}`
  (provider_catalog → ProfilesDocument);AckStatus 路径 `qaqh_client::ringing::…`
  (v2.rs:24 把 qaqh_ringing 重导出为 ringing 模块)

### M12 分组管理(workspace.move_session / detach)
- runtime 语义实锤:两命令都是 **CommandTarget::Session**(workspace.rs:114-136:
  `session_target(target)`),移动/脱离的是 target 会话
- bridge:`MoveSession{seed,workspace_id}` / `DetachSession{seed}`
- UI:分组头 hover 显现「移入」(`.group("grp")` + `.group_hover("grp", opacity 1)`,
  点击 `cx.stop_propagation()` 防折叠冒泡);会话头「脱离」chip;
  `WorkspaceGroup` 增 `id: Option<String>`(未分组为 None,移入时 unwrap_or_default → 移到 ""?
  **注意**:未分组头的「移入」会发空 workspace_id,daemon 侧应会拒绝——留给 runtime 验证,
  若报错在 UI 侧禁用即可)


## 第三轮新增(M8/M9/M10)

### M8 RingingV2 daemon 命令面(bridge.rs + store.rs + ui.rs)
- **新增 BridgeRequest**:`CreateSession{cwd,effort}` / `PatchSessionSettings{seed,effort,tool_mode}`
  / `SetPermissionLevel{level}` / `ListProviders` / `QueryConfig` / `DeleteSession{seed}`
  / `Watchdog{seed,after_ms}`
- **新增 BridgeMsg**:`Created(seed)`(session.create 同步 ack.result.session_id → 自动 select)
  / `Config(Value)` / `Providers(Value)` / `Watchdog(seed)`
- `send_command` 现返回 `Option<RingingV2CommandAck>`(ack.result/revision 载荷可取);
  ack 类型路径:`qaqh_client::ringing_v2::RingingV2CommandAck`(根导出没有!)
- SDK 类型经 qaqh-client 根导出可用:`SessionSettingsPatch`/`ToolsSettingPatch`/
  `SessionCreateResult`/`CommandPrecondition`(lib.rs:45-46)
- UI:
  - 侧栏「新建」→ 新建会话面板(工作区循环切换,cwd=workspace.path;effort 默认/低/中/高)
  - 会话头 chips:强度(默认→低→中→高循环)+ 工具(默认→全开→禁用)→
    `session.settings.patch`(本地回显,真值由 daemon 事件对账)
  - 「删除」二段确认(第一次武装/第二次执行)
  - 设置页新增 **daemon 区**:权限档位三段(config.load 的 `permission_level`,wire
    三档 1 只读/2 工作区写/3 跳过授权;乐观更新本地快照)+ provider.list 只读投影
    (数组或 {providers:[…]} 两种形状);打开设置页即发 QueryConfig+ListProviders

### M9 可靠性
- **取消看门狗**(electron 模式):cancel 后桥线程 tokio 计时 5s → BridgeMsg::Watchdog →
  DaemonStore 检查该会话仍 Running → `reducer::clear_transient` + `Refill` 重灌快照
  (UI 永不假死;`arm_cancel_watchdog` 在 cancel 监听器里武装)
- **发送四态禁用**:`SessionStore::send_block_reason()`(连接断开/有待应答交互/回合
  运行中)与 composer 提示同源:placeholder 显示原因、非聚焦时边框转 warn 色

### M10 打磨
- 代码块内容横向滚动(`overflow_x_scroll` 在 StatefulInteractiveElement 上,
  **plain Div 没有——必须先 `.id(...)`**;代码内容 div id=("mdcode-content",…))
- TodoRow.id 死字段删除(warning 清零)

## 第二轮新增(M6/M7)

### M6 设置页(已实现)
- `src/settings.rs`:本地偏好。模型照搬 zed 设计(借鉴不抄):**UI 无状态、
  读写全落文件、变更 → refresh_windows**。分层 = 代码默认值 + 用户 JSON
  (`#[serde(default)]` 缺字段自动落默认)。
  - 路径:`$XDG_CONFIG_HOME` 或 `~/.config` 下 `qaqh-gpui/settings.json`,tmp+rename 原子写
  - 字段:`appearance{theme_mode,density}` / `typography{ui_font_size,ui_font_family,
    code_font_size,prose_line_height,content_width}` / `behavior{stream_follow,output_tail_max}`
  - `settings::update(cx, f)` 是唯一写入口(改内存→落盘→theme::apply_mode→refresh_windows)
- 设置页 UI(`settings_page` in ui.rs):侧栏「设置」按钮开关;外观/排版/行为/daemon
  四区;控件:三段选择器、步进器、循环切换、字体族输入框(IME 可用)
- **主题模式真值在 settings**:`ThemeState` 只持 {appearance, palette};
  app.rs 里 settings::init 必须先于 theme::init
- serde 契约测试 `settings_json_contract`(roundtrip + 旧文件缺字段兼容)

### M7 字体/布局调优(已实现)
- **rem 缩放**:DaemonStore::render 每帧 `window.set_rem_size(ui_font_size)`
  (text_xs/sm/base 全是 rems(),一处设置全 UI 等比缩放)
- 行高/内容列宽/密度(gap shift ±2px)/代码块头栏+复制按钮/输出尾窗
  `utf8_tail`(16KB UTF-8 安全截断+「仅显示输出尾部」)
- **流式自动滚底**:`timeline_scroll: ScrollHandle` + `follow_tail`;
  `.overflow_y_scroll().track_scroll(&handle)`;`set_offset(point(0,f32::MAX))`
  (paint 钳制,div.rs:2539);`on_scroll`(原 on_scroll_wheel)上滚暂停/下滚恢复

## 侦察结论存档
- **qaqh-electron**(webui/SolidJS):设置页=浮层页;持久化两层(daemon config.save /
  本地 localStorage);排版:正文14px、行距 24/14 解耦、内容列 760px、气泡 max 85%、
  间距 4/8/12/16/24/32;UX 珍品:取消看门狗(store.ts:1108-1146)、发送四态、
  follow-tail、回合导航刻度轨、侧栏拖宽、已工作X秒、退出三段确认
- **zed**:settings_ui=静态数据页+pick/write+渲染器注册表;设置=defaults+user 合并、
  写文件+refresh_windows;rem=set_rem_size;DynamicSpacing Base00-48;
  行高:默认 phi(1.618)、markdown 段落 1.3;内容列宽 850px
- RingingV2 命令全集在 `../qaqh-backend/crates/qaqh-ringing/src/command.rs`
  (ts-rs 生成);QueryRequest 白名单只管 `service_call`(siwc/dsaccount),
  **`service_v2` 不设客户端闸门**(v2.rs:399),config.load/session.list 直走

## 项目定位

QAQ-Harness 的 GPUI 桌面前端,宪法:**RingingV2 协议与命令面最高准则**,前端零自建
协议、零转换层(`qaqh-client`/`qaqh-ringing-contract` 跨仓 path 依赖)。

- gpui 依赖:crates.io `gpui-pre` / `gpui-pre-platform` `=0.3.11` + 组件库 `gpui-component` `=0.9.0`(Apache-2.0;2026-11-08 起弃用 `../gpui` path vendor,该检出已删除)
- `../zed` = 上游克隆;**GPL crates 只借鉴设计,不可抄代码**
- `../qaqh-electron` = 另一前端(electron 壳 + webui/SolidJS)
- 资源:8 核老 U / 7.7G 内存(可用 ~3.4G)→ **永远 `-j1`**

## 文件地图(M1-M10)

| 文件 | 内容 |
|---|---|
| `src/bridge.rs` | 桥线程:命令面(CreateSession/PatchSettings/SetPermissionLevel/ListProviders/QueryConfig/DeleteSession/Watchdog)+ 名册轮询 + 事件路由 |
| `src/store.rs` | SessionStore(+effort/tools_mode/confirm_delete/send_block_reason/arm_cancel_watchdog)+ DaemonStore(+config/providers/new_session_*/create_session) |
| `src/ui.rs` | 全部 UI;新建会话面板/设置页(含 daemon 区)/会话头 chips/删除二段/发送受限提示 |
| `src/theme.rs` | Palette 19+6 槽 × 亮/暗;apply_mode;ActiveTheme |
| `src/settings.rs` | 本地偏好持久化(XDG json)+ Density + Typography 快照 + 契约测试 |
| `src/text_input.rs` | TextField(UTF-16)+ EntityInputHandler(SessionStore/DaemonStore)+ InputElement<V> |
| `src/markdown.rs` | pulldown-cmark 事件流→块;ParseCache;行高/代码字号/复制/横向滚动 |
| `src/highlight.rs` | tree-sitter-highlight 7 语言 + 内容寻址缓存 |
| `assets/queries/*.scm` | nvim-treesitter highlights.scm ×7(Apache-2.0) |
| `src/reducer.rs` | 事件 reducer;clear_transient(含 md_cache.clear) |
| `src/app.rs` | settings::init → theme::init(顺序!);appearance observer |
| `docs/HANDOFF.md` | 本文件 |

## 关键 API 事实(gpui fork / SDK 实测)

- `Window::set_rem_size`(window.rs:3349);text_xs/sm/base=rems() → 全局缩放
- `Window::handle_input` 只能 paint 阶段;自定义 Element 包 AnyElement 用其固有方法
- `HighlightConfiguration::new` **5 参**(lang,name,highlights,injections,locals);
  `highlight(config,bytes,None,|_| None)`;configure 先调
- pulldown-cmark 0.13.6:Tag::Item/TableRow/TableCell unit 变体;Event::Rule 独立;
  InlineMath/DisplayMath 必须覆盖
- 滚动:`.overflow_y_scroll()`/`.overflow_x_scroll()` 在 **StatefulInteractiveElement**
  (plain Div 没有,先 `.id(...)`);`.track_scroll(&handle)` 配对;
  ScrollHandle{offset,max_offset,set_offset};paint 钳制 [-max,0](div.rs:2539);
  `on_scroll`(原 on_scroll_wheel);ScrollDelta::Pixels 用 `f32::from`(.0 私有)
- 剪贴板:`cx.write_to_clipboard(ClipboardItem::new_string(String))`
- `RingingV2CommandAck` 路径:`qaqh_client::ringing_v2::RingingV2CommandAck`
  (根导出没有);ack.result/revision 可取;CommandOptions.preconditions 可带
  CommandPrecondition::SettingsRevision/ConfigRevision(乐观并发,本期未接)
- SDK 根导出:`RingingCommand/CommandTarget/SessionSettingsPatch/ToolsSettingPatch/
  SessionCreateResult/CommandPrecondition`(lib.rs:45)
- listener 闭包第 4 参是 `cx: &mut Context<V>`(必须命名);`.child(&str)` 要 'static
- Context<T> Deref→App;`on_click` 无 listener 版:`Fn(&ClickEvent,&mut Window,&mut App)`

## 许可证红线

- 可抄:gpui-pre / gpui-pre-platform / gpui-component 全部(Apache-2.0)、zed assets/themes 色值(MIT)
- 只可借鉴:`../zed` GPL crates(含 settings_ui)
- 本仓:queries 为 Apache-2.0(assets/queries/README.md)

## 验证命令

```
cargo check -j1     # 0 error,唯一 warning=windows-capture patch(有意)
cargo test -j1      # 5/5:ask×3 + settings 契约 + highlight 资产冒烟
cargo build -j1     # 链接 ~12s
```

## 遗留(下一轮候选,按价值排序)

1. provider/profile 编辑(provider.upsert/ProfileCreate;electron registry.ts 模式;
   api_key 三态 Keep/Set/Clear)
2. 时间线 `list()` 虚拟化;回合导航刻度轨;「已工作X秒」
3. 侧栏拖宽 + 宽度记忆;附件/截图;siwc/dsaccount 接入页(SERVICE_METHODS 白名单已通)
4. 未分组头「移入」的空 workspace_id 处理(见 M12 注意)
5. 修复会话头 chips 的本地回显与 daemon 事件的对账(effort/tools/model 真值回推)

