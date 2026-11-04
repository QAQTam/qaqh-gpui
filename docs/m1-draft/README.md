# M1 增量草稿(等 M0 `cargo check -j1` 绿后移入 src/)

移动方式:
```bash
cp docs/m1-draft/{bridge.rs,store.rs,reducer.rs,ui.rs,app.rs,main.rs} src/
rm src/runtime.rs
```
另需在 Cargo.toml [dependencies] 增加:

```toml
qaqh-ringing-contract = { path = "../qaqh-backend/crates/qaqh-ringing-contract" }
```

(直接持有权威契约类型,非转换层。)
