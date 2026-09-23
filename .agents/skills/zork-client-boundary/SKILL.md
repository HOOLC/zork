---
name: zork-client-boundary
description: 开发、重构或审查 zork 客户端功能及 Rust core/UI 分离时使用；检查 GPUI、Android/JNI 的业务操作、状态、网络与平台适配边界，防止业务逻辑回流 UI。
---

# 客户端业务边界

先读 [core/UI 合同](../../../docs/design/client-core.md)；该文档规定职责，实际调用点查源码。订阅改动另用 [zork-client-subscriptions](../zork-client-subscriptions/SKILL.md)，Chat 用 [zork-chat-messages](../zork-chat-messages/SKILL.md)，文件树用 [zork-shared-files](../zork-shared-files/SKILL.md)。

## 开发与审查

- 沿实际调用链核对：用户输入 → core 业务操作 → 服务/存储 → 状态发布 → 平台应用。UI 只负责呈现、动画、输入和瞬态交互状态；校验、默认值、权限、业务集合、网络、持久化、重试、取消与操作生命周期属于 `zork-client-core`。Rust 文件或名称含 core 不自动证明合规。
- UI 提交业务意图、消费只读投影。通用 `Request(method, path, body)` 转发不算业务接口；点击回调不手工增删第二份业务列表，不从 outbox 消失推断送达或从断线推断未执行。
- 输入缓冲、焦点、选区、滚动、离场动画元素和派生绘制缓存可留在 UI。合法性、可执行动作及持久草稿规则由 core 返回；不能把只读呈现映射误判为另一份业务权威。
- GPUI 与 Compose 的视觉组件不承担业务判断。平台原生控件的选择不授权 UI 绕过 `zork-client-core` 处理操作或状态。
- 复用同一设备/会话的控制器。异步结果绑定实际操作、设备/会话和连接代次；关闭组件不代替业务取消，旧结果不能写入新上下文。并发编辑在权威写入边界检查预期版本，不能整包回写旧列表。
- 平台适配只提供 core 定义的系统能力，不承担业务策略；core 独立于 GPUI、Compose 和视图生命周期。桌面与 Android fixtures 共用公开合同，原生设计应用也不复制业务规则。
- 导航、资源目录、通知与长正文附件转换复用各自 core 合同。通知接受回执不等于已读；秘密输入不进平台保存状态；手机设置范围见 [Android 设置](../../../docs/design/interface.md#mobile)，不因界面对齐复制电脑端节点管理职责。

## 验证

先运行 `python3 scripts/check-client-boundary.py`；改检查器时加 `--self-test`。它检查依赖、UI IO 和业务请求，但不能证明异步所有权、状态归并或并发编辑正确；不以改名、移文件或宽泛白名单绕过。

按 [zork-validation](../zork-validation/SKILL.md) 选择 core 行为和平台映射用例，检查最终状态及取消、重连、切换、撤权等受影响边界。只修本次范围；约束改变时更新合同，完成状态和验证结果随任务报告，不维护调用点迁移表。既有违规不是新代码的豁免，也不自动要求全仓整改。
