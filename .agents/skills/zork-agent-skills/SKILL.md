---
name: zork-agent-skills
description: 编写、审查 zork 内部 Agent 的 Skill，或开发、排查其发现、来源绑定、分发版本与回滚时使用。维护仓库的 Codex 项目 skill 文案不自动触发运行时改造或回归。
---

# Agent Skill 写作与运行时

先读 [来源与管理合同](../../../docs/design/agent-skills.md)，工具接入读 [外部能力](../../../docs/design/external-capabilities.md)。Skill 文件使用与配置合同变化时同步内置 `crates/agent/skills/skill-management/SKILL.md`。

仓库 `.agents/skills` 指导开发者，不会自动分发给产品内 Agent。文件归属和共享交付的运行时指引维护在 `crates/agent/skills/file-sharing/SKILL.md`，与真实工具、内置分发和模型前发现一起验证。

## 写作边界

编写或审查前，按 [内容职责](../../../docs/design/agent-skills.md#content-boundary) 区分 Skill 的最佳实践与 `tool.help` 的工具手册。帮助缺失时补工具帮助，不把接口用法放进 Skill 正文、示例或 references 补位。

## 发现与消费

- 候选是真实文件或 `synch://` 引用，同名保留，以完整来源与内容哈希定位，不设名称覆盖优先级。单个文件或管理记录损坏只隔离该项并保留诊断，不阻止其它 Skill 和节点启动，不生成虚拟正文。
- Skill 独立 space，Agent 通过通用文件 API 读取，用户通过专用 UI 查看，不进入共享文件入口。来源配置按合同合并分发、用户、共享、节点和 Agent 来源；按通用目录 API 分页，发现到 manifest 即停止深入资源；资源不占候选预算，保留额度、远端等待边界和续读信息。
- 节点专用来源不自动进入其他节点清单。当前 Mesh 完全互信，显式文件读取与自动 Skill 发现分开；不从共享文件 UI 推导物理目录或新增私有隔离。
- 使用实际执行节点的配置。附加来源通过现有 Agent 配置入口修改，不增加专用来源工具。模型请求前刷新，变化才持久记录，压缩/交接后重新提供；不空闲轮询、改旧历史或自动运行脚本。
- 复制、修改与读取使用通用文件工具，来源引用进入既有 Agent 配置。`file.read` 处理正文/资源，拼相对路径时放在查询串前，保留 origin 和 snapshot、只去掉 manifest 文件的 root。执行需本地路径时才按 [共享文件](../zork-shared-files/SKILL.md) materialize。

## 修改与分发

- 修改遵循通用文件工具的版本检查，保留现有正文、资源和并发工作；分发选择由软件管理，不复制一组 Skill 文件包装工具。
- 归档后的资源示例不再被发现。分发历史先排除再计扫描额度，但已选旧版本路径仍可显式读取。
- 分发状态与不可变版本完整校验后原子切换。普通只读当前视图可重建，不重复发现或绕过停用；发布中断可恢复，用户同名目录和改动不覆盖。每个 Skill 的状态与不可变历史位于其自身 `.zork/`，更新和回滚不改变其它 Skill。
- 定制复制完整目录到普通来源再修改。更新不自动合并、移动或备份用户副本；只读分发不是 OS 安全沙箱。
- 停用按稳定分发目录 ID 保留；为保留用户修改而更换目录时，继承该 Skill 的选择和历史，旧版本路径仍可读。回滚先检查外部修改，只选完整版本；同内容重启继续生效，不同分发内容到达后才恢复升级。

按 [验证规则](../zork-validation/SKILL.md) 选择发现、管理、bundle 和历史前缀测试。进程测试先重建 Station，入口为 `scripts/test-skills.py`，远端引用用共享文件 fixture；假模型不证明真实模型遵循指引。
