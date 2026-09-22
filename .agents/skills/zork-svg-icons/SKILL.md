---
name: zork-svg-icons
description: 设计、修改或审查 zork 功能 SVG 图标及跨端资源接入时使用；统一造型、视觉重量、圆润度和语义复用。品牌标志、头像与供应商 logo 保留各自规范，控件外框圆角另见 zork-rounded-corners。
---

# 功能 SVG 图标

先查 [图标规范](../../../apps/zork-design-pc/docs/12-icons-and-scenes.md)、[共享资产](../../../crates/zork-ui/assets/) 和调用点。同义符号优先复用，旧资源仍注册不等于产品当前默认。

## 造型

- 常规基准为 24×24 viewBox、约 1.7 描边、round cap/join；已按小尺寸校准的资产先看效果，不为统一属性批量缩放。
- 圆润落实到内部路径和实心几何，外层按钮圆角不能消除图形尖角。保留箭头、折页等语义结构，按实际尺寸检查居中、留白、笔画与负空间。
- 单色图标使用 `currentColor`，线与实心部分明确 fill。品牌、头像和供应商 logo 保留原造型、色彩与许可；控件外框另用 [圆角 skill](../zork-rounded-corners/SKILL.md)。
- 图形与热区分开，纯图标按钮有可访问名称；状态和可执行性由 core 提供。空态优先使用规范中的现有符号与文字。

## 接入与验证

原生客户端与 `zork-design-pc` 共用资产注册与 `controls::icon`，不在页面复制 SVG；Android 通过 [导出脚本](../../../scripts/android/export_design_assets.py) 消费共享路径，核对实际输入与支持的元素/变换，不手工重画。触屏热区按 [移动端设计](../../../apps/android/design.md)。

维护来源、manifest 和许可；原生设计应用的嵌入资源由 `scripts/design-pc/generate_assets.py` 更新。单图标修改不能靠生成脚本的成功代替实际绘制检查。

检查 XML、viewBox、填充/描边、变换与注册；再按 [UI parity](../zork-ui-parity/SKILL.md) 看实际使用尺寸、DPR、邻近图标和状态，确认无裁边且视觉重量一致。文件存在、注册成功或放大母版不代表实际显示通过。纯 skill 修改按 [验证规则](../zork-validation/SKILL.md) 检查文案与链接。
