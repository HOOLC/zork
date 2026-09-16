这些只读卡片快照由 Rust core 提供，来自 `headless_interactions` 的离线样例，Android 测试不自行推导业务结果。

更新方式：运行 `cargo test --locked -p zork-gui --features headless-bench --test headless_interactions` 后，将 `artifacts/interactive-messages/native/message-interaction-input-wide.json` 和 `input-accepted.json` 分别复制到`assets/` 中的 `interaction-ready.json` 与 `interaction-completed.json`。
