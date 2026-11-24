<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->
项目宪法:
0. 本仓输出的命令禁止出现shell的节流命令和超时命令。Harness的shell自带超时和自动节流。此举是为了满足用户想要看shell实时输出，exec提供了真实的pty前端展示。
1. 禁止为了做一个简单功能写一堆测试
2. 为了快速上线,模型必须要遵守用户可能带来的破坏性改动,并且不允许拖泥带水
3. 遇到架构冲突时,原则上以更好的架构推行,即使用户可能想反悔--不允许!必须要开快车
4. 如果遇到兼容性架构,必须砍掉!因为不再建议为了兼容套一群兼容性代码
5. 不允许写防御性代码:不能工作视为代码错误应该改正而非写补丁让它看起来能工作
6. 允许在开工前分析GitHub同类项目的哪些架构值得参考并且尝试告诉用户，并且有需求时尽量至少调用一次codegraph cli
7. 开工时必须要获取用户的disk usage/memory usage/cpu核心,并且确认资源受限的情况下优先和用户沟通接下来可能出现的卡顿
8. 不得因为工具错误但是用户心切默认声称已经完工
9. 针对本项目:QAQHarness的RingingV2协议和命令面视作最高准则,任何模块,包括agentloop必须为它让路,不得出现拧巴设计(为了声称兼容ringing而写一个转换层,这类属于懒政,必须批判)
10. 针对ClaudeCode/Codex:不允许改一次代码做一次全局测试,磁盘空间受不了.
11. 优先使用ApplyPatch/apply_patch进行修改;应该注意:rg -r==rg --replace，DO NOT USE rg -rn
12. 使用sed修改文件必须参带rg确保修改正确.
13. 必须在工作前使用todo工具让用户知道你的修改流程
14. 涉及到新的库时可以用find-docs skills去找最新API避免旧版用法性能低下或不可用
15. 若本仓库为后端LTS仓，需要遵循用户空间（前后端API）不可变，内核空间（后端）保持对外一致或同等效力下可以发生部分小破坏性变化或以exp设计接线。
16. 本仓禁止直接使用git checkout，可以使用更加安全的新用法。
17. shell字段包含pwsh和bash。调用时禁止调用powershell5，批判性调用cmd。
18. 若本仓为前端electron仓，需要确保修改后端功能时优先走LTS修改。
19. 针对QAQHarness，工具侧有handoff，tool result也有上下文长度。在用户未指定或明确规划长程任务时，handoff阈值在上下文窗口达到36%-45%之间，确保手头工作完成后调用交接功能自动换新对话继续执行。不得超长。
20. 前端视觉立法:qaqh-gpui 一切外观/UI 改动必须遵守 docs/DESIGN-LAW.md（词汇表+布局预算+验收清单），验收按清单逐条执行。硬规则四条:①icon-first——持久控件（侧栏入口/列表动作/状态指示/composer 工具条）一律用内嵌 Lucide（gpui_kit_assets::IconName，全集已在图内）+ tooltip，新增文字 chip 即违规，文字只许做表单标签/tooltip 正文/二级 meta；②字阶五档封顶（页标题/行标题/meta/微注/数据），raw id 与 ULID 禁止作为展示标题，只许进 tooltip 或 meta 行；③密度预算——侧栏顶部功能区固定 8 行高度、会话行高≤28px、首屏无滚动可见≥10 行会话、每屏 accent≤2 处；④间距走 4px 网格、圆角只用 kit 三个 RADIUS token、颜色只走 Palette 语义槽，裸 hex/rgb 一律打回。
