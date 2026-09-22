# zork-design-pc

原生 Zork 设计应用由 `crates/zork-gui` 的 `zork-design-pc` 可执行程序提供。左侧目录同时浏览真实 Rust/GPUI 组件、首次使用流程、设计规范和素材；应用与桌面客户端共用 `zork-ui` 的控件实现。

在仓库根运行：

```sh
python3 scripts/design-pc/build.py --verify
pnpm design:pc
```

`docs/` 是可编辑设计规范，`assets/` 与 `tokens/` 保留素材和参数源；`licenses/` 保留第三方来源。`mobile/`、`motion/`、`wordmark/` 和 `previews/` 保存原始设计输入。`archive/` 保存早期网页原型和固定对照截图，仅供追溯，不参与应用运行。素材清单中的状态和摘要决定当前方向，历史参考不能冒充客户端实际界面。

新增组件时，先在 `zork-ui` 或对应客户端页面实现完整控件，再在原生设计应用的故事目录中用相同入口提供隔离参数。更新本目录素材后运行 `python3 scripts/design-pc/generate_assets.py`，让可执行程序嵌入最新的获选资源。
