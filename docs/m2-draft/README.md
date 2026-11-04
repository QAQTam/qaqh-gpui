# M2 增量草稿(composer 发消息 / 权限与 plan 应答 / 中断)

等 M1 build 绿并完成 probe 验证后应用:

```bash
cp docs/m2-draft/{bridge.rs,store.rs,ui.rs} src/
scripts/mem-guard.sh 900 cargo check -j1   # 增量,~1-2 分钟
```

M2 范围:
- `BridgeRequest` 新增 SendMessage / RespondPermission / RespondPlan / Cancel,serve 循环转 `RingingCommand::…` 经 `send_command_v2_typed` 直发。
- `SessionStore` 增加 composer 文本 + FocusHandle(轻量 key_down 输入框,gpui 0.2.2 无内置 Input 组件)。
- 交互卡片按钮:Permission(允许/拒绝)、Plan(批准/拒绝);Ask 先展示 choices(M2.5 再接 AskAnswer.question_id 解析)。
- header 在 `ActivityState::Running` 时显示「中断」(conversation.cancel)。
