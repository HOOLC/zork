# 移动端规范

当前生产界面以 [Android 设计](../../android/design.md)和[Chat 导航](../../../docs/design/chat.md#navigation)为准。nav7 原型保存在[历史原型](../archive/mobile-prototype/index.html#nav)，只用于追溯当时的布局输入。

## 平台差异

| 项目 | 桌面 | Android |
| --- | --- | --- |
| 主布局 | 侧栏与内容并排 | 导航、对话分别独占一屏 |
| 导航行高 | 32 px | 48 dp |
| 独立操作命中区域 | 按桌面密度 | 常用控件至少 44 dp |
| 悬停反馈 | hover 背景 | 按压期间显示同一语义背景 |
| 聊天列表选中 | 展示当前对话 | 不保留持续 selected 背景 |
| 输入 | Enter 发送、Shift Enter 换行 | Enter 换行，点击发送按钮提交 |
| 返回 | 内容与侧栏并存 | 保留来源、草稿和阅读位置 |

各设备的 Chat 按真实消息时间混排，并以设备标识标明执行设备；新建 Chat 只有一个悬浮入口。手机设置分客户端、Mesh 与高级三组，不提供工具连接、消息折叠或旧 Agent 定义的创建、编辑和授权管理。历史 Chat 和任务保持可读。

桌面数值不是全平台硬性尺寸。移动端采用 [Android 设计](../../android/design.md)与 [mobile-tokens.json](../tokens/mobile-tokens.json)，不直接套用可拖动侧栏或常驻选中背景。
