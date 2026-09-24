# Agent Skill

Skill 是运行时 Agent 可按需读取的本机指引文件。当前只支持本机 Skill：不跨设备共享、不经 Mesh 分发，没有发布、版本历史、远端引用或启用开关。

<a id="content-boundary"></a>

## 内容职责

- `tool.help` 是工具手册：用途、调用格式、参数与返回值、标识符语义、约束、错误处理，以及分页、上传等接口要求的步骤。具体合同由工具实现维护，不在 Skill 中另存一份。
- Skill 是最佳实践：何时采用某种做法、怎样判断取舍、如何组织任务和检查结果，以及领域陷阱。可以指向 `tool.help`，但不复制参数表或操作手册。

区分依据是内容职责，不是篇幅。接口规定的多步调用仍属于手册；缺少调用说明时补 `tool.help`，不以 Skill 代替。

## 文件与优先级

Skill 是一个目录中的 `SKILL.md`（frontmatter 含 `name`、`description`）及其相对引用的资源。位置都在 Agent 数据根的 `skills/` 下：

- 内置 Skill 随版本编译进程序，启动时写入 `skills/bundled/<name>/`，只读。内容与本版本不一致（缺失、被改动或版本变化）时整体替换，一致时不写盘。
- 用户 Skill 位于 `skills/<name>/`，是普通文件，Agent 用通用文件工具创建、修改和删除。隐藏目录与保留名 `bundled` 不作为用户 Skill。
- 身份是 frontmatter 的 `name`。用户 Skill 与内置 Skill 同名时替换内置项；删除用户副本即恢复内置版本。同层同名只取路径排序第一项并给出诊断。
- 旧分发系统直接装在 `skills/<name>/` 且带 `.zork/.state.json` 的副本会在启动时移除，避免被当成用户覆盖。

## 清单

每个普通模型请求前重新扫描一层目录，生成紧凑清单：名称、描述、`SKILL.md` 绝对路径和用户 Skill 目录，不含正文。清单变化时才追加为持久 notice，不改写已发送前缀；压缩或交接后的新 generation 重新提供当前清单。只改正文不改变清单，模型按需用 `file.read` 读取最新正文。

单个文件缺失 frontmatter、格式错误、超过 128 KiB 或重名时只跳过该项，并在清单中列出原因；不影响其它 Skill 和节点启动。

## 验证

`zork-agent` 的 `skills` 单元测试覆盖解析、发现、覆盖、刷新与内置写入；`zork-agent-testkit` 的 `skills` 测试确认模型请求携带清单、变化后追加且前缀不变、交接后重新提供。假模型只证明调用链，不能证明真实模型遵循指引。开发入口见 [zork-agent-skills](../../.agents/skills/zork-agent-skills/SKILL.md)。
