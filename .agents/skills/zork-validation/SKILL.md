---
name: zork-validation
description: 为 zork 代码改动选择并执行回归验证，或诊断构建和 CI 失败；涵盖后端、进程边界、桌面验收及独立性能基准。
---

# 按改动选择验证

遵循 AGENTS 和本地环境规则，先读当前 package scripts、workflow 与测试入口。冻结依赖；进程测试必须使用本次重建的二进制。纯文案只查结构/链接/事实，个人设备安装不自动增加回归。

用户明确批准的关键要求统一由 [关键门禁](../zork-critical-gates/SKILL.md) 管理；代码交付及合并、发布验收运行其 smoke suite，不在普通回归清单中另存阈值或通过状态。

## 选择范围

Rust 先测受影响 package，JS 用对应测试，完整 CI 以当前 workflow 为准。客户端另跑 `scripts/check-client-boundary.py` 并审查实际业务调用链；静态扫描或截图不能证明边界正确。

分支与 PR 不自动运行 CI；提交和推送前使用仓库 hooks 完成对应验证。main 只承担必要的运行时契约与部署输入检查，其他回归留在本地；不为替代本地验证而反复触发远端任务。发布和已批准关键门禁仍按各自入口执行。

- 桌面普通逻辑：`pnpm test:desktop`。完整桌面/发布验证：`scripts/test-desktop-headless.py`；先读覆盖，避免重复运行。原生显示性能用 `scripts/test-desktop-performance.py`，需要解锁桌面。
- 启动恢复与聊天可用：`scripts/test-desktop-startup.py --app /path/to/Zork.app`，覆盖已有 Mesh 身份、返回导航、用户发送及 Agent 实际回复回到客户端，以及离线缓存、准备失败与重试。首屏、节点 ready、用户消息送达或模型收到输入都不能代替完整聊天往返；同时覆盖 Mesh 未就绪时的本地回复，记录首屏后的操作时间。缓存探针只读，正常退出与强制终止分别取证，不把强杀后的恢复冒充普通重启。它不更改关键门禁的阈值。
- Agent/Station：[执行合同](../../../docs/design/agent-runtime.md) 及相关 mailbox、嵌入与升级测试；生产 Agent 在 Station 内。[Chat 接续回归](../../../scripts/test-agent-continuation.py) 覆盖结束确认、后台持久投递和失败呈现；选择真实模式时显式提供私有 Profile。真实供应商验证使用显式提供的私有 Profile 和隔离 Session/workspace，覆盖生产调用链的工具往返、后续 turn 与重启回放；裸 HTTP 成功不能代替该链路验收。[Go 真实回归入口](../../../crates/agent-testkit/tests/opencode_live.rs) 默认不运行，不向在用会话提交测试输入。
- 用户参与：[合同](../../../docs/design/user-participation.md)、`scripts/test-user-interactions.py` 与 core 测试。分别验证不含业务载荷的登记合同和业务卡片链路；用普通生产 `agent.create/update` 核对原 invocation 的实际效果、局部修改、重复/取消和恢复，不能用通用等待探针代替。
- 导航/历史：[Chat](../../../docs/design/chat.md#navigation)、[历史](../../../docs/design/execution-history.md)、`scripts/test-chat-navigation.py`；文件和 Skill 按 [共享文件](../zork-shared-files/SKILL.md) / [运行时 Skill](../zork-agent-skills/SKILL.md) 选入口。
- Android：`scripts/android/build.py` 与对应 instrumentation，遵循 [手机范围](../../../docs/design/interface.md#mobile)。通知 fixture 只在独立模拟器运行。
- 液态/文本：[UI parity](../zork-ui-parity/SKILL.md)，CPU/GPU 报告用 `scripts/storybook/test_liquid_frame_budget.py` 匹配同产物。
- macOS 身份：`scripts/test-macos-notification-identity.py`、`scripts/test-macos-process-identities.py`。主入口与实际 GUI 必须同签名身份，deep 签名通过不保证 OS 接受；横幅/声音/冷启动点击另测，不改正式应用权限。
- DeepSWE：`benchmarks/deep-swe/README.md` 与 `pnpm benchmark:deep-swe:test`；适配器测试不证明模型评测通过。

选定脚本前读其当前参数与 fixture 范围。

## 渲染性能

改动消息、Markdown、代码、列表、选择、Tooltip、图片或动画时检查受影响热路径。正式 GUI 交付包含性能证据，保存同主机/配置/窗口/输入的前后基线；构建与测量分开。

- 消息压力为 **100000 条、全部当前呈现类型混排**，含长中英、代码、批注、文本/文件/图片附件及投递/活动状态。覆盖开头/中段/末尾、冷进入与往返滚动，展开历史文件列表；测字号时确认设置生效。
- 只构建可见内容，记录实际载入、解析、可见/构建数和内存。离屏用 `zork-gui-render-bench`；全量回放设 `ZORK_SCROLL_ALL_MESSAGES=1 ZORK_BENCH_MESSAGE_COUNT=100000`，原生用 `scripts/test-desktop-performance.py --messages 100000`。
- 门槛查当前脚本，不降负载、放宽阈值或关闭要求的功能。报告 p95/p99、负载与未覆盖范围；CPU、模拟时钟、GPU 和显示 FPS 分开。`Application::run` 若直接退出，须提前写报告/断言并验证失败退出码。

## 构建与收尾

- 按 [构建指南](../../../docs/guides/rust-builds.md) 加载 `.env`，优先现有封装；直接命令前用 `eval "$(python3 scripts/lib/build_env.py --shell)"`。显式环境优先，Cargo 不自动加载 `.env`。
- 独立 target 放配置根的 `isolated/<任务名>`，不另建 `/tmp` 大缓存。软回收阈值不是硬配额；先预览，确认不在用后清理，不绕锁或清空其他任务缓存。
- 每 worktree 保留一份 `.tmp/macos-app.noindex/Zork.app`，打包脚本加锁更新。新包成功才替换，失败留旧包，原来运行则重启；普通更新不导出历史副本，显式导出才用 `--output`。
- 用 finally/trap 结束本任务实例，确认不用后注销临时注册并清理额外 app、staging、CEF 副本与独立 target。固定当前 app、正式安装、用户数据/身份、报告及他人产物保留；明确需要的候选/回滚包压缩归档。

交付注明验证的源码/产物、结果与缺口，区分 fixture、真实设备和公开发布物；不将测试计数追加进常驻设计。
