---
name: zork-agent-skills
description: 编写、审查 Zork 运行时 Agent 的内置 Skill（crates/agent/skills），或开发、排查本机 Skill 的发现、覆盖、清单注入与内置写入时使用。仓库 .agents/skills 的开发者 skill 不属于运行时。
---

# 运行时 Agent Skill

合同见 [Agent Skill](../../../docs/design/agent-skills.md)。实现在 `crates/agent/src/skills.rs`，清单由 runner 在普通模型请求前注入；Station 基础提示只说明清单存在，不列 Skill 正文。

## 写作

- 内置 Skill 正文在 `crates/agent/skills/<name>/SKILL.md`，并在 `skills.rs` 的 `BUNDLED` 登记；目录名与 frontmatter `name` 一致，语言与其它内置 Skill 保持一致。
- 按 [内容职责](../../../docs/design/agent-skills.md#content-boundary) 只写判断与取舍；工具参数、返回与步骤属于 `tool.help`，缺失就补工具帮助。
- 引用的工具名必须存在于当前工具目录（`crates/agent/src/session/tools.rs`、`crates/agent-station-tools`）。工具改名或下线时同步检查所有内置 Skill 和 `crates/station/prompts`。
- Skill 文件使用方式变化时同步内置 `skill-management`。

## 边界

- 只有本机 Skill：不恢复 `synch://`、共享/远端来源、发布、版本历史或启用开关，除非用户重新决定。
- 内置目录只读且由版本整体替换；定制通过同名用户 Skill 覆盖，不在内置目录修改。
- 单个坏文件只产生诊断，不阻止其它 Skill 或启动；清单只在变化时追加，不改写已发送前缀。

## 验证

`zork-cargo test --locked -p zork-agent --lib skills` 与 `zork-cargo test --locked -p zork-agent-testkit --test skills`；改动基础提示或工具名时另跑 `-p zork-station` 相关测试。
