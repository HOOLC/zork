---
name: zork-ui-parity
description: 新增或修改 zork UI 控件、共享组件及 zork-design-pc 展示，或维护原生客户端与 Android 的视觉和交互一致性。
---

# 共享组件与跨端对照

按范围读 [界面设计](../../../docs/design/interface.md)、[组件包](../../../crates/zork-ui/README.md) 和 [手机设计](../../../apps/android/design.md)。圆角见 [圆角 skill](../zork-rounded-corners/SKILL.md)，图标见 [图标 skill](../zork-svg-icons/SKILL.md)。

## 复用与输入

- 所有小组件默认宽度随内容和内边距变化，高度由实际内容布局决定。只有设计合同或调用场景明确指定尺寸时才固定宽高；文字、语言或内容变化后重新测量。按 [界面设计](../../../docs/design/interface.md#视觉与组件) 核对共享组件、调用点和展台。
- 桌面优先使用 GPUI、`gpui-base`、`gpui-component` 的按钮、选择、菜单、浮层和定位；Android 使用 Compose Material3。`zork-ui` 持有共享语义和桌面组合，页面与 `zork-design-pc` 调用同一生产控件，不另写展示版。业务数据与意图遵守 [client boundary](../zork-client-boundary/SKILL.md)。
- 保持 Zork 的颜色、尺寸、文字角色和状态。hover/pressed、selected、invalid、busy、disabled 和键盘焦点分别检查；禁用输入不能只改颜色。触屏关键动作不能依赖 hover。
- 输入复用 `ComposerInput` 与宿主键盘映射。Composer Enter 提交，独立多行字段 Enter 换行；IME 候选不提交。只读和禁用同时约束键盘、IME 与粘贴，投影回声不重置光标和撤销历史。
- 模态、菜单与浮层检查 Esc/Back、正反 Tab、外部点击、滚动、IME 和焦点返回。GPUI 可聚焦按钮已将 Enter/Space 变成点击，避免在 key-down 重复执行。弹窗关闭后不可见内容不保留输入资格。
- macOS 应用外壳共用菜单动作；退出走 GPUI 优雅关闭，设置走客户端现有导航，编辑命令复用共享输入动作。分别验证菜单点击和快捷键。

## 展示与验证

- 组件改变时按 [展台约束](../../../docs/design/interface.md#component-gallery) 检查相同控件在生产页和 `zork-design-pc` 的实际状态。示例使用 mock 参数，不复制业务规则。
- `scripts/design-pc/build.py` 构建设计应用；按影响选择 `test_package.py`、`test_native_workbench.py` 和原生交互测试。通过真实入口检查宽/窄窗口、中文、长文案、键盘、焦点、滚动与裁切。
- Android 检查安全区域、底部表单、系统返回和 IME；用隔离模拟器验证交互，真机刷新率与功耗另行取证。模拟器帧率不代表真机呈现。
- 受影响的客户端完整帧按 [验证规则](../zork-validation/SKILL.md) 与 `scripts/smoke-critical.py` 执行。组件目录需覆盖页面和目录的真实滚动、PlainDialog 的内容和背景中间帧，以及生产会话和十万条混合消息。编译、截图、离屏 Metal 完整帧和屏幕实际 FPS 分别报告。
