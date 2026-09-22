# 项目文档

项目是什么、有哪些组成部分，先看 [README](../README.md)。这里按文档的用途分层；同一层中的文件再按所约束的对象命名。接口细节看源码与生成帮助，任务进度和一次性验收不进入长期文档。

## 设计约束

`design/` 说明系统应当怎样工作，以及不能破坏的语义；不代表每项设计均已完成实现。

- **执行与配置**：[Agent 运行时](design/agent-runtime.md)、[模型连接与账号](design/model-connections.md)、[Agent Skill](design/agent-skills.md)。
- **协作与事实**：[Chat](design/chat.md)、[活动与执行历史](design/execution-history.md)、[用户参与](design/user-participation.md)。
- **数据与能力**：[设备身份和生命周期](design/devices.md)、[外部能力与授权](design/external-capabilities.md)、[共享文件](design/shared-files.md)、[同步与恢复](design/synchronization.md)。
- **客户端**：[core/UI 边界](design/client-core.md)、[状态订阅](design/state-subscriptions.md)、[系统通知](design/notifications.md)、[界面与组件设计](design/interface.md)。

## 待决方案

`proposals/` 只保存仍需决定或验证的方案，不能当作现有能力说明。落地后将必要约束归入对应设计，移除实施过程。

- [跨端插件体系](proposals/plugin-system.md)：同包 PC/Android、业务/UI 分离，后端与 ABI 待实验确定。
- [独立手机推送](proposals/mobile-push.md)：通道准入和后台投递待验证，与现有前台服务连接分开。

## 操作指南

`guides/` 说明怎样完成具体工程操作；规则引用设计，命令不散落在设计文件中。

- [Android 本机操作](guides/android-local-operations.md)：脚本使用方式、原生入口与权限条件。
- [Rust 构建与缓存](guides/rust-builds.md)
- [共享文件 CAS 审计与回收](guides/file-cas-recovery.md)：原数据保护、只读预览与引擎回收前提。
- [原生节点安装、接入、发布与升级](guides/native-releases.md)
- [release/dev 开发、自救与提升](guides/release-dev-recovery.md)

原生设计应用的运行与诊断在 [组件包指南](../crates/zork-ui/README.md#playground)，平台开发说明位于各应用 README。视觉资产与交互参考见 [zork-design-pc](../apps/zork-design-pc/README.md)，移动端几何见 [Android 设计](../apps/android/design.md)。

开发规则和仓库内容边界见 [AGENTS.md](../AGENTS.md)；项目 skills 提供任务入口和检查方法，不另存一套设计或实现清单。
