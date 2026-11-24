# qaqh-gpui 视觉设计法（DESIGN LAW v3.0 · v5.0.0）

> AGENTS.md §20 的现行执行细则（v3.0 融合版）。
> 架构与美学立场：**Overcoder v5.0 骨架 × Antigravity 中性克制美学 × GPUI 120fps GPU 物理动效**——Win11 原生 Mica 底壳、纯原生 Rust 渲染（零 WebView）、克制中性灰/暖纸/深石墨色阶、无塑料感、全界面可打断物理动画。

## 0. 核心设计原则

1. **中性克制，内容为王**：告别高饱和冷紫蓝/大红大蓝实心钮。默认提供双风格预设——`WarmPaper`（默认，Overcoder 暖米白纸感舞台 `#F6F5F0`）与 `PureStudio`（Antigravity 纯白极简 `#FFFFFF`），均采用深石墨主强调色（亮色 `#18181B` / `#111111`，暗色 `#F4F4F5`），仅在状态与危险处点缀语义色。
2. **思考与工具轨迹隐退**：默认态下思考块与连续成功工具组以**无边框纯灰紧凑行 + hover 淡底**呈现，不抢正文叙事焦点；仅在展开时以 `MotionReveal` 物理高度手风琴展开 1px 垂直脊柱。移除正文每段下方重复刷屏的 `tok/s` 尾注。
3. **不打断工作流的模态设置**：设置页从“双侧栏套娃占位”改为**居中悬浮模态窗**（`Presence` 物理进退场 + 压暗遮罩 + Esc/点击遮罩关闭），保留主对话/欢迎舞台在底层；全项采用统一 `segmented_strip` 分段滑块与 `Switch`。
4. **物理连续动效纪律**：所有可交互折叠/抽屉/浮层统一由 `qaqh-gpui-kit::motion` 驱动（`Spring`、`Presence`、`MotionReveal`、`Stagger`），静止零重绘、中途切换保持速度连续；提供 `Full`（流畅物理）/ `Reduced`（精简动效）/ `Off`（零动效）三档，且自动响应系统 `reduce_motion()`。
5. **RingingV2 真值唯一**：状态、命令、事件直持 `qaqh-gpui-wire` 契约，零转换层；`ses_*` / ULID 禁止直接泄露至展示标题位。

## 1. 表面与光学层级

| 层级 | 语义槽 | WarmPaper (Light) | PureStudio (Light) | Dark (Warm / Pure) | 用途 |
|---|---|---|---|---|---|
| **L0 窗口材质** | `chrome` | `#F4F4F6` (`a=0.80`) | `#F6F6F7` (`a=0.80`) | `#111113` (`a=0.84`) | TitleBar 与侧栏透出 Win11 Mica |
| **L1 主舞台** | `stage` | `#F6F5F0` (`a=1.0`) | `#FFFFFF` (`a=1.0`) | `#191816` / `#161619` | 主对话舞台、代码/终端面板 |
| **L2 卡片与胶囊** | `card` / `surface_subtle` | `#FFFFFF` / `#EFEDE7` | `#FFFFFF` / `#F4F4F6` | `#21201D` / `#1E1E22` | Composer、设置分组卡、分段底槽 |
| **L3 浮层** | `card` + `shadow_popover` | `#FFFFFF` | `#FFFFFF` | `#21201D` / `#1E1E22` | 模态设置窗、Popover、右键菜单 |

- **主色与反色槽**：`accent` + `on_accent`（亮色 `#18181B` 配 `#FFFFFF`；暗色 `#F4F4F5` 配 `#111114`），确保所有主发送钮与主按钮高对比度。
- **行内代码槽**：`code_bg`（暖纸 `#EAE9E2` / 纯白 `#F0F1F3` / 暗色 `#26262B`）与舞台底色形成温和清晰的阅读对比。
- **禁止业务代码硬编码裸 `rgb(...)` / 十六进制颜色**：全部走 `qaqh_gpui_kit::theme::Palette`（终端 ANSI 调色与测试常量除外）。

## 2. 几何、圆角与阴影

- **4px 间距网格**：`4 / 8 / 12 / 16 / 24 / 32 px`。
- **圆角梯级（`kit.rs`）**：`RADIUS_STAGE = 18.0`、`RADIUS_COMPOSER = 18.0`、`RADIUS_BUBBLE = 16.0`、`RADIUS_CARD = 14.0`、`RADIUS_POPOVER = 12.0`、`RADIUS_CONTROL = 10.0`、`RADIUS_CHIP = 8.0`。
- **轻量化阴影**：缩小扩散半径与墨色浓度，靠 L0→L1→L2 明度差与极细 `border_subtle` 建立层次，告别厚重浮雕。

## 3. GPU 物理动效规范（`qaqh-gpui-kit/src/motion.rs`）

- `spring_f32`：接入 `gpui_kit::base::motion::spring`，驱动侧栏、Inspector、底部交互终端坞开合（静止自动收敛零帧请求，中途折返速度连续）。
- `presence_overlay`：接入 `gpui_kit::base::motion::Presence`，驱动居中模态设置窗真实进/退场与反向中断。
- `accordion_reveal`：结合 `spring_f32` 与 `MotionReveal`，驱动侧栏工作区分组、时间线思考块、工具轨迹脊柱的实测高度平滑展开与收起后零开销卸载。
- `spring_chevron`：Lucide `ChevronRight` 通过 `SPRING_CHEVRON` 实现 `0° ↔ 90°` 顺滑旋转。
- `stagger_enter`：基于 `Stagger` + `animate_keyframes` 为列表/卡片提供 32ms 错峰入场。

## 4. Wave 1 验收清单

1. [ ] Workspace 版本号为 `5.0.0`；`ThemePreset`（`WarmPaper` / `PureStudio`）与 `MotionLevel`（`Full` / `Reduced` / `Off`）向后兼容旧 `settings.json`。
2. [ ] 亮/暗双预设呈中性灰白黑/暖纸质感，无冷紫蓝塑料色与大红大蓝实心钮；Composer 发送钮亮色近黑白箭头、暗色纯白黑箭头。
3. [ ] 设置以居中悬浮模态窗呈现，支持 `Presence` 进退场动画、Esc 与点击遮罩关闭；底部「重连本地 Daemon」完整可读无截断。
4. [ ] 侧栏去闲置灰点；工作区分组、思考块、工具轨迹组具备手风琴 + 旋转箭头动画，终端标签简名化（`pwsh`/`bash`/`cmd`）且底部终端坞支持弹簧平滑开合。
5. [ ] `cargo check -j 2`、`cargo check --features acceptance -j 2`、`cargo test -p qaqh-gpui-kit -j 2` 全部零错误通过。
