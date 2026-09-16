# 原生资源接入与对照

本目录保存图形资源接入映射和固定截图基线。当前应用入口为 `DesktopRoot`；功能状态以 [桌面节点](../../../docs/design/devices.md#lifecycle)、[Chat 导航](../../../docs/design/chat.md#navigation) 和 [执行历史](../../../docs/design/execution-history.md) 为准。旧直接 Station 入口、全局 Inbox/看板和独立 Drive 已退役，不再维护两份迁移清单。

## 资源映射

共享图标、字标与控件由 `crates/zork-ui` 提供，应用视图负责组合；当前视觉合同见 [已确认设计](../docs/gui-approved-design.md)。资源的来源与许可证继续随资产保存。

| 基线中的动效 | 接入位置 |
| --- | --- |
| 图标与联动 | 首位伙伴的对话引导、主会话品牌位 |
| Zork 字标 | 设置侧栏页脚 |
| 圆润弹开/跳落、变形字标 | 首位伙伴创建引导 |

[资源挂载清单](native/usage-v2.json)记录这批固定基线中的资源与位置，不代表后续版本的完整使用清单。

## 固定对照材料

[正常动效帧](native/brand-motion/frames.json) · [减少动态帧](native/brand-reduced/frames.json)

[宽窗口会话](native/conversation-comments-1280x800.png) · [窄窗口会话](native/conversation-comments-900x600.png) · [文件预览](native/conversation-file-preview-900x600.png)

这些材料保留历史视觉依据，不证明当前安装版本、源码测试或设备性能。新改动按 [UI parity](../../../.agents/skills/zork-ui-parity/SKILL.md) 选择对照与验证，实际测试报告放入本地 artifacts。
