# zork-design-pc

原生 Zork Design 由 `crates/zork-gui` 的 `zork-design-pc` 可执行程序提供，用于开发时快速查看某个组件在某个状态下的效果。它只呈现真实 Rust/GPUI 组件与模拟数据，与桌面客户端共用 `zork-ui` 的控件实现；设计规范在本目录的 Markdown 中阅读和修改，不在应用内浏览。

在仓库根运行：

```sh
python3 scripts/design-pc/build.py --verify
pnpm design:pc
python3 scripts/design-pc/build.py --package-app '.tmp/Zork Design PC.app'
```

`docs/` 是可编辑设计规范，`assets/` 与 `tokens/` 保留素材和参数源；`licenses/` 保留第三方来源。`mobile/`、`motion/`、`wordmark/` 和 `previews/` 保存原始设计输入。`archive/` 保存早期网页原型和固定对照截图，仅供追溯，不参与应用运行。素材清单中的状态和摘要决定当前方向，历史参考不能冒充客户端实际界面。

打包入口使用 `crates/zork-ui/assets/app/icon-design.svg` 生成的 `ZorkDesign.icns`，只写入不存在的目标路径，保留原有 App 供比较或回退。

新增组件时，先在 `zork-ui` 或对应客户端页面实现完整控件，再在 `crates/zork-gui/src/desktop/stories.rs` 的目录中登记它的产品区域、状态与源码路径。
