# Zork loading SVG

独立 SVG 设计资源，配套 `preview.html` 可直接在浏览器打开。颜色与线条来自 `src/design.rs` 的 ZORK_UI 和现有图标；companion 原样复用品牌路径。

| 文件 | 场景 | 推荐尺寸 | 周期 |
| --- | --- | --- | --- |
| ring.svg | 默认加载、按钮、文件 | 16–24 px | 1 s |
| ticks.svg | 紧凑状态行 | 16–20 px | 1 s |
| dots.svg | 对话等待 | 24 px | 1.5 s |
| connect.svg | 设备连接 | 24 px | 1.8 s |
| companion.svg | 启动准备 | 40–64 px | 2.4 s |

## 使用

- SVG 透明底，使用 currentColor。内联 SVG 可继承父容器颜色；以 img 引用时不能继承宿主 color，需要通过内联或生成指定颜色版本处理。建议默认 #24272B，辅助状态 #646970。
- 每个文件包含独立 CSS 动画和 prefers-reduced-motion 静态回退。所有动画仅改变 transform 或 opacity。
- 加载信号旁保留具体状态文字；装饰用途的内联 SVG 设 aria-hidden="true" 并移除 role/aria-label，由外层 status 文本提供可访问名称。区域加载时设 aria-busy，完成后清除；不要每帧播报。
- 同一区域只显示一个加载信号。建议等待约 200 ms 再出现，避免短请求闪烁；有真实百分比时使用确定进度，不用这些图形暗示进度。
- 原生客户端使用共享 `components/loading.rs`：通用圆环与对话三点。`native-ring.svg` 是无 CSS 的静态几何，旋转和透明度由 GPUI 驱动；隐藏、完成或减少动态效果时停止持续刷新。其他三款仍是设计备选。
- preview.html 是设计资源展示页；实际控件来自 GPUI loading stories。原生生命周期与性能证据位于仓库 `artifacts/loading/`，WASM 渲染需单独验收。
