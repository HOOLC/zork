---
name: zork-rounded-corners
description: 实现、调整或审查 zork 控件外轮廓的圆角、描边与内容裁切时使用；优先复用 GPUI 或 Compose 原生形状并检查小尺寸与跨端一致性。SVG 图标内部造型另见 zork-svg-icons。
---

# 圆角、描边与裁切

尺寸与视觉语义按 [界面设计](../../../docs/design/interface.md)，Android 触控与底部表单按 [手机设计](../../../apps/android/design.md)。图标内部路径另用 [图标 skill](../zork-svg-icons/SKILL.md)。

- 先选择现有控件和半径变体。按钮、字段、卡片、浮层分别按其语义使用 GPUI `rounded` / `overflow_hidden` 或 Compose `RoundedCornerShape` / Material3 `Surface`；不要为普通圆角维护 Bézier 路径、独立绘制层或物理模拟。
- 填充、描边、hover、focus 与内容裁切使用同一轮廓。GPUI 的 `overflow` 只提供矩形子内容视口；滚动内容需要圆角裁切时，对滚动层施加真实路径掩码，不能用底色覆盖边角。焦点和错误改变颜色而不加粗边框；嵌套行保留足够内边距。
- 宽高缩小时将半径限制在短边一半以内。无框预览保持矩形视口，图片或透明背景需要真正的内容裁切，不能靠父底色遮盖。
- 浮层由 `gpui-base::Positioner` 或 Compose 的菜单、Dialog、ModalBottomSheet 放置，空间不足时翻转或限高滚动；触发器、面板和可访问焦点身份保持稳定。
- 用受影响的真实页面和 [设计应用测试](../../../scripts/design-pc/test_native_workbench.py) 检查短边、长文案、描边四角、焦点、内部滚动与窗口边缘。跨端组件同时按 [UI parity](../zork-ui-parity/SKILL.md) 验证。
