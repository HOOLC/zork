# Skill 目录来源

Agent 自动发现配置目录中的 `SKILL.md`。默认来源为节点数据目录下的
`skills/`；可以把它改为共享盘挂载目录。此版本使用执行设备上的文件系统路径，
不把 Mesh 的逻辑 space 自动挂载或同步成文件系统。

推荐布局：

```text
shared/
  skills/                    # 所有节点共同配置的默认来源
    coding/SKILL.md
  devices/
    desktop/skills/           # 仅 desktop 节点配置
    laptop/skills/            # 仅 laptop 节点配置
```

## 节点与 Agent 配置

节点的 `config.json` 增加 `skills`，其它配置保留：

```json
{
  "skills": {
    "shared_path": "/mnt/shared/skills",
    "paths": ["/mnt/shared/devices/desktop/skills"]
  }
}
```

`shared_path` 缺省为 `skills`，`paths` 缺省为空。所有相对路径都以节点数据目录为
基准，与任务工作目录无关；不展开 `~` 或环境变量。多台设备的挂载点可以不同，
各自配置本机能访问的路径。目录不存在时报告诊断，不创建目录或复制文件。

创建 Agent 的 `POST /v1/node/agents` 可带 `skill_paths` 字符串数组。
已有 Agent 使用以下节点管理 API，需要节点管理认证：

| 接口                              | 请求／响应                                                                                  |
| --------------------------------- | ------------------------------------------------------------------------------------------- |
| `PUT /v1/node/agents/{id}/skills` | `{"paths":["/mnt/shared/team/skills"]}`；保存并返回 Agent 定义                              |
| `GET /v1/node/agents/{id}/skills` | 返回 Agent 的 `paths`、合并后的 `sources`、发现的 `catalog.skills` 和 `catalog.diagnostics` |

PUT 替换该 Agent 的额外路径列表，空数组清除额外来源，仍继承共享和设备来源。
每份额外路径列表最多 32 项，路径不能是空白或包含控制字符。配置持久化，
修改不改变模型、指令、身份或任务归属。当前通过配置文件和 API 设置，尚无专用设置界面。

来源包括当前分发版本中启用的 skill 目录、本机 `custom-skills/`、共享来源、节点 `paths` 和 Agent `skill_paths`。
同名文件全部保留，不设名称优先级或覆盖关系。名称是标签，规范化后的完整路径用于定位；
重复来源、重叠目录和目录符号链接指向同一个规范路径时只列一次，记录首先发现它的来源。
清单按名称、路径稳定排序，排序不代表推荐优先级。由 agent 根据任务、设备、描述和来源
选择候选，必要时读取多个正文比较。共享盘上可见的其它设备目录不会自动成为来源；
不要把所有设备目录共同的父目录加入默认来源。

Leader 和 Worker 各自使用执行节点保存的 Agent 配置。远程委派不会把 Leader
的路径配置带给 Worker。Worker 的持久 session 分配用于在重启后找到自己的配置。
不属于节点 Agent 的普通会话加载分发、共享和设备来源。

## Skill 文件与发现规则

```markdown
---
name: build-project
description: 构建项目，并检查当前设备所需的工具。
---

先阅读项目的构建说明。相关脚本位于 scripts/build.sh。
```

支持单层 skill 目录、分组子目录，也支持直接把包含 `SKILL.md` 的目录作为来源。
发现 `SKILL.md` 后停止深入该 skill 的资源子目录。隐藏子目录和符号链接子目录
不自动遍历；显式配置的来源根可以是目录符号链接。`SKILL.md` 自身必须是普通文件。

Frontmatter 必须以 `---` 开始和结束，并提供非空 `name` 与 `description`。
首版解析字符串子集：单行普通字符串、JSON 风格双引号字符串、YAML 单引号字符串、
缩进的 `|`／`>` 多行说明；忽略其它 metadata。暂不解析 YAML anchor、tag、
复杂对象或跨行引号字符串。名称最长 128 字节且不能含空白、路径分隔符和控制字符，
说明最长 1024 字节。单文件最多 128 KiB；一次发现最多 256 个 skill 文件、4096 次
目录／条目检查、8 层子目录，最多返回 32 条诊断。触及限制时返回已有结果和诊断。

## 自动加载与按需读取

每次普通模型请求前重新读取配置并扫描目录。清单包含名称、用途、来源、文件路径和
内容摘要哈希；只有变化时才把新清单追加到持久会话记录，不改写已发送的历史前缀。
正文变化也会更新哈希。上下文压缩或交接完成后的普通请求会重新提供当前清单。
空闲会话不启动目录轮询，模型无需读取所有正文。

- `skill.list({})`：重新发现当前会话适用的 skill，返回清单和诊断。
- `file.read({"path":"清单中的完整路径"})`：使用通用文件工具读取正文；按照返回的
  `next_offset` 分页直到结束。引用文件以 `SKILL.md` 所在目录为基准读取。

这些工具沿用现有 `call`／`tool.help` 协议。文件缺失、格式错误、来源读取失败或
删除后的 skill 请求产生可见诊断／工具失败，不让其它有效 skill 或聊天不可用。
读取 skill 不会自动运行脚本。

## Agent 自助管理

所有 skill 都是真实文件，没有 `skill.read`、虚拟 skill URI 或按名称取正文的分支。
程序启动时校验随应用分发的完整文件清单，将正文和资源放入
`bundled-skills/.versions/<版本目录>/`。全部写入和校验成功后，原子更新
`bundled-skills/.state.json` 的活动版本；扫描器只发现当前版本中启用的目录。
分发版本的标识由文件路径和内容哈希生成，应用升级但 skill 内容未变时不重复安装。
更新失败不会把半成品设为活动版本，旧版本目录保留供回滚和已有文件路径读取。

分发文件设为只读，`skill.write`／`skill.archive` 拒绝修改发行版来源。需要定制时复制
整个 skill 目录到 `custom-skills/`、共享或其它合适的来源，赋予副本所有者写权限后修改。
同名副本全部列出，不设置名称优先级；普通 `file.read` 仍可读取分发文件。
这是发行目录的管理约定，不是针对拥有本机文件权限的用户的安全沙箱。

定制流程是复制整个 skill 目录到普通来源、修改并验证副本，再停用对应的内置 skill。
更新器只管理发行版本，不自动备份本地修改、不合并副本、不迁移旧目录。
已有普通文件和定制副本不由更新器搬动或删除。版本完整性校验仍用于拒绝损坏包和不完整回滚目标。

停用按稳定的分发目录 ID 保存，与名称和版本无关。升级、重启、回滚不会重置它。
开发者应保持同一个分发 skill 的目录 ID 稳定，修改名称或正文无需改变 ID。
旧版本保留在本机，不自动删除用户副本或历史版本。管理工具可选择一个完整旧版本
回滚；同一分发内容版本重启后保持该选择，收到不同的分发内容版本才恢复正常升级。

来源配置出错时报告诊断，不从二进制内容生成虚拟 skill 候选或读取结果。

| 工具             | 作用                                                                                                                       |
| ---------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `skill.bundle`   | `list` 查询活动／历史版本与停用项；`disable`／`enable` 加 `skill` 修改本机停用状态；`rollback` 加 `version` 切换完整旧版本 |
| `skill.sources`  | `action: "list"` 查询有效来源；`add`／`remove` 加 `path` 原子增删当前 Agent 的额外来源                                     |
| `skill.validate` | 校验完整 `content`，返回 metadata 与内容哈希，不写磁盘                                                                     |
| `skill.write`    | 指定 `source`、相对 `directory`、完整 `content`；更新已有文件必须提供当前 `expected_hash`                                  |
| `skill.archive`  | 指定 `source`、`directory`、当前 `expected_hash`，归档正文并返回 `archived_path`                                           |

来源管理按调用会话的实际归属定位 Agent，覆盖本地及远程 Worker；参数不能指定另一个
Agent。节点共享／设备默认来源仍由节点配置管理。没有归属 Agent 的普通会话只能查询
来源，修改时返回明确错误。添加路径幂等，删除使用查询结果中的原始配置路径，不复制
或删除文件，也不修改其它 Agent 的配置。

写入与归档限定在当前配置的来源里。来源根必须已经存在，以免共享盘离线时误建本地
替代目录。`directory` 可以包含分组目录，但不能越界、包含隐藏路径或穿过符号链接，
也不能写进另一个 skill 的资源子树。直接以 skill 目录为来源时，`directory: "."`
可更新／归档该目录的现有 skill；创建普通新 skill 则使用子目录。

工具使用每个 skill 目录里的文件锁协调写入，检查旧内容哈希，并通过临时文件与原子
重命名发布完整正文。已有文件缺少哈希或哈希已过期时拒绝覆盖；应重新读取、保留并发
修改后提交新版本。支持文件锁的共享盘上，多进程使用这些工具也遵循此约定；普通
`file.write` 或外部编辑器不参与这套锁协议。

归档将正文移动到 skill 内部的 `.skill-archive/`，保留资源；没有活动正文的归档目录
不继续递归发现资源中的示例 skill。归档结果返回的文件可用 `file.read` 读取，再以
`skill.write` 恢复活动正文。归档不会删除旧版本。工具返回更新后的文件清单，便于检查
新增、更新、归档的具体路径以及其它同名候选。`skill.write` 只管理 `SKILL.md`，附属脚本和资源继续使用
现有文件工具管理。

## 验证

```sh
python3 scripts/lib/build_env.py -- cargo test --locked -p zork-config --lib skill_tests
python3 scripts/lib/build_env.py -- cargo test --locked -p zork-agent --test skill_bundles --test skills --test skills_management
python3 scripts/lib/build_env.py -- cargo test --locked -p zork-agent-testkit --test skills
python3 scripts/lib/build_env.py -- cargo test --locked -p zork-station --bin zork-station skill_
python3 scripts/lib/build_env.py -- cargo build --locked -p zork-station
python3 scripts/lib/build_env.py -- python3 scripts/test-skills.py
```

测试覆盖配置兼容、同名候选保留、路径去重、设备来源范围、资源子树、符号链接、损坏／超限文件、
通用文件分页读取、分发更新、回滚、停用、独立定制副本保留、来源撤销、动态清单、持久历史前缀和重启恢复。进程测试使用隔离数据目录、
真实 Station 和假模型，不验证跨物理设备挂载或真实模型是否会正确遵循 skill。

## Skill 与工具帮助的职责

Tool manuals belong in tool.help: invocation syntax, parameters, results, identifiers, constraints, errors and required API procedures such as pagination or uploads. Skills contain reusable decision criteria, trade-offs, task organization, quality checks and domain pitfalls. Do not hide API manuals in Skill references; missing help should be fixed at the tool.
