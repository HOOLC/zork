---
name: zork-ui-parity
description: 修改 zork 共享组件，维护 GPUI 原生/Web 与 Android 组件对照，或验证已有设计的视觉及交互一致性。
---

# 共享组件与跨端对照

按范围读 [界面设计](../../../docs/design/interface.md)、[组件包](../../../crates/zork-ui/README.md) 或 Web/GUI README；几何用 [圆角 skill](../zork-rounded-corners/SKILL.md)，资源用 [图标 skill](../zork-svg-icons/SKILL.md)。

## 复用与输入

- 液态的数值、呈现适配和后端缓存按 [呈现分层](../../../docs/design/interface.md#liquid-presentation) 管理；GPUI 完整控件在 `zork-ui`，Android 完整控件由 Compose 适配共享结果，应用与图库只组合。Web 编译同一 Rust/GPUI 实现。业务数据与意图按 [client boundary](../zork-client-boundary/SKILL.md)，fixtures 不复制业务规则。
- Playground 全部 UI 也复用完整现有组件。另写后移进库、增加外壳专用 API，或只共享几何、内部按钮和正文均不算复用；提取公共组件时同时迁移原示例，核对双方的材料、目标切换、退场保留、输入和焦点生命周期。卡片、预览边界和内容面板默认由共享实际布局定高，调用者不传预估高度；多个触发器分别拥有动画来源和返回焦点。
- 沿共享 tokens、资源和本地化入口，不翻译用户内容/后端错误。动画复用 motion，中断接续当前状态；明确的物理模型按其合同，离场元素不成为业务列表。
- 输入复用 `ComposerInput` 与宿主键盘映射，明确 Composer 提交与独立多行字段换行的 Enter 语义。相同 `set_value` 回声保留光标/历史，恢复其他字段用 `reset_value`；IME 候选不提交。只读和禁用状态同时约束键盘、IME 与粘贴；折叠退场内容同步禁用后代 Tab 停靠点。
- 模态和自绘示例共用 `modal::trap_focus` / `navigation::move_focus`，同时覆盖原始按键和编辑器动作。来源被遮挡或离屏时，组件仍持有其焦点句柄，打开移交内容、关闭回到来源。显式 `FocusHandle` 须同步其 `tab_stop`，仅设元素属性无效；共享元素在布局前保留稳定自动化 ID。GPUI 可聚焦点击控件已将 Enter/Space 转为 `ClickEvent::Keyboard`，不在 key-down 再执行同一次操作。
- 液态的材料身份、绘制归属、生命周期和视觉配方按 [液态合同](../../../docs/design/interface.md#liquid) 检查。对照开合中间帧的连接、分离与融合，以及真实背景和遮暗层上的边框、实色遮挡、图标按钮反馈、主操作配色与焦点/错误状态。来源的持续呈现和单次合成须覆盖透明填充、文字边缘、父级透明度、缓存回退及快速反向；输入区域存在不能代替像素验证。滑动反馈另比较短、中、长距离的速度、耗时及中途反向。core 接受意图后的临时气泡不证明消息送达。

## 对照与性能

- 目录区分基础组件与业务组件。按 [展台约束](../../../docs/design/interface.md#component-gallery) 核对项目实际使用的全部业务组件类型：项目和 Playground 调用同一完整入口，只替换真实/mock 参数与事件适配，不保留两套组件实现。参考库用于核对职责和行为合同；覆盖与验证结果随任务保存，不维护常驻组件状态表。
- 完整图库用 `scripts/storybook/build.py`，Web 用 `build_web.py`；读取当前选项和锁定工具链。导出失败保留上一套完整原生产物，全部成功后才更新图库与 manifest，避免部分新图混入旧图。平台问题局部配置，不改全局链接。
- 按影响选择 `test_package.py`、`test_web.py`、`test_playground.py` 或具体交互脚本。通过真实入口展开、滚动、选择，覆盖宽/窄窗口、中文、键盘/IME、焦点与裁切，不直接改 fixture 跳过操作。布局验收分别量取内层内容和外层卡片的末项、底边与侧边，覆盖换行和内容增删；只检查内层面板不能证明外层留白正确。
- 外壳与示例的导航、浮层和触发器用 `test_liquid_overlays.py` 检查形变、反向、按压、平移缓存与停帧。另用 `test_liquid_lifecycle.py` 在分组页面检查来源的唯一输入身份、操作开始时切到目标层及首帧像素一致、整个面板内部实色与外部裁切、来源文字的遮挡、关闭过程与关闭后滚动、锚点绘制与命中同步、菜单转移和嵌套边缘；覆盖窗口边缘与离屏返回。按液态合同允许复制绘制结果，不能复制交互对象。透明退场内容不挡再次激活，模态换内容后焦点仍有效。换层首帧用 `test_liquid_transfers.py` 比较打开、关闭及双向反转前后的完整画面；快照以实际已提交显示的帧为准。用 `test_liquid_settling.py` 核对可见运动结束后的残留调度与展开中途的背景显露。静态截图和代码调用关系不能代替动效验收。
- 解析、高亮、文本布局和轮廓随内容/尺寸缓存；绘制子树的复用须覆盖父级透明度与裁切变化，不能缓存住隐藏时的颜色；缓存命中后仍须保留测量阶段创建的控件状态与身份。预热后保留隐藏内容时，同步撤销鼠标、Tab、无障碍和自动化输入资格；输入许可不能冻结在缓存里，完全隐藏后不再驱动动画帧。仅重绘动画时保留原输入树，精确命中读取当前轮廓；真实内容、输入或窗口变化须使旧绘制快照失效并返回正常布局。只构建可见行；选择与命中避免平方成本，静止不持续刷新。原生 test-support 在 effects 刷新时绘制 dirty window；确需手工绘制的底层测试通过 App 更新窗口，不能在已借用根 View 的更新闭包内重入 draw。
- Android 由可见宿主统一调度液态场景，变化输入与数值结果批量传递；帧通道不走业务 JSON。[场景协议](../../../crates/zork-liquid/src/scene.rs) 与平台解码同步更新，内容和遮暗使用共享的独立呈现输出，不复用材料进度或在 Compose 重写相同运动。文字/IME、无障碍、窗口焦点和显示时钟留在平台。模态位置、安全区域及 IME 布局按 [手机设计](../../../apps/android/design.md) 检查。检查静止、离屏、后台与减少动态效果，不把模拟器帧率或桌面 CPU 基准当作手机功耗证据；入口见 [Android 指南](../../../apps/android/README.md)。
- 按 [验证规则](../zork-validation/SKILL.md) 执行受影响的十万条混合消息/附件及渲染门禁。原生验收同时核对真实缓存复用、掩码更新及回退，并用 `scripts/storybook/test_native_liquid_clip.py` 检查换层首帧像素一致及中途、反向的裁切；静态截图不能代替路径证据。液态 Web 固定 release 产物、实际后端、视口与 DPR，独立测 CPU/GPU。
- Playground 性能回归用 `test_native_playground_performance.py` 和 `test_liquid_workload_performance.py`（均在 `scripts/storybook/`），覆盖目录、完整对话框开合及页面、目录内部滚动；保留首次输入和全部长帧。Web 分项数据与原生完整帧分别报告，关键门禁范围与阈值以 `scripts/smoke-critical.py` 的 `GATES` 为准。按用户实际访问源核对自动选择的后端，不能用 localhost 的结果代替其他访问源。默认浏览器调度用于交互对照，取消限速的压力采样和带探针的诊断数据另列，不能混用样本。Web 帧调度另用 `test_web_gpu_backpressure.py` 验证完成通知延迟或失败时的有界提交、输入/尺寸恢复与静止停帧。

报告实际平台与输入/呈现覆盖；编译、静态截图、离屏 CPU 和物理 FPS 各自取证。纯文案修改只做文档检查。
