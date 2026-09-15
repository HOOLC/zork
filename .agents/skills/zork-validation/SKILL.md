---
name: zork-validation
description: 为 zork 代码改动选择并执行回归验证，或诊断构建和 CI 失败；涵盖后端、进程边界、桌面验收及独立性能基准。
---

# 按改动选择验收

以下路径均相对仓库根目录。先遵循 `AGENTS.md`，再读取 `package.json`、当前相关 workflow 和改动模块的测试入口。以当前脚本和实现核对文档，历史状态文档不是现行 CI 清单。

- 使用 `package.json` 的 pnpm 版本和冻结锁文件，Cargo 使用 `--locked`。工作区依赖 feature 调整另读 `docs/rust-build-cache.md`，避免无意引入编译变体。
- Rust 逻辑先验证受影响 package；后端集成使用 `pnpm test:rust`。JS 行为用对应测试或 `pnpm test`。完整 CI 以 `.github/workflows/ci.yml` 的事件条件为准：PR 运行行为回归，静态检查放在本地 pre-commit 与 main；删去重复或低价值检查，复用同一组运行时 fixture 构建。不声称未运行的项目通过。
- 客户端功能或 core/UI 边界改动先运行 `python3 scripts/check-client-boundary.py`，并按 [zork-client-boundary](../zork-client-boundary/SKILL.md) 复查入口、业务操作、状态发布和 UI 订阅的完整链路，再选择相关 core 行为与平台映射验证。共享组件依赖检查或界面测试通过，不能证明应用层没有业务逻辑、网络请求或第二份业务状态；纯视觉调整不因此扩大为全套业务测试。
- 进程测试使用构建产物。运行前先重建涉及的二进制，通常为 `cargo build --locked -p zork -p zork-station -p zork-agent-server -p zork-gh`。不要用旧产物验证新源码。
- Station/Agent 嵌入、生命周期、更新或消息边界改动：读取 `docs/zork-agent-status.md` 最新对应段落，再选择 `test/merged-runtime.e2e.test.ts`、`test/gateway-mailbox.e2e.test.ts`、`crates/zork-gui/tests/test_gateway_entry.py`、`scripts/test-embedded-gateway.py`、`scripts/test-native-upgrade.py`、`scripts/test-gateway-upgrade.py` 中相关回归。Station 内嵌 Agent；独立 Agent binary 的存在不表示生产 supervisor 有第二个 Agent 子进程。
- 桌面普通逻辑使用 `pnpm test:desktop`；用户明确要求桌面完整验证或正式发布验证时，复用 `python3 scripts/test-desktop-headless.py`。个人测试设备的安装不自动触发该流程，遵循本地环境 skill。它包含共享组件、渲染、选择与模态框、安装器及进程合同测试，并在打包前重建不带 benchmark feature 的产物。先检查脚本，避免重复跑它已覆盖的全部测试。
- 原生显示性能任务才使用 `scripts/test-desktop-performance.py`，需要可用的解锁桌面。headless 确定性回放、硬件 CPU 耗时、显示器 FPS 是不同证据，不能互相替代。
- 存储、查询、运行时性能修改及发布前的性能验收：从 `docs/zork-agent-status.md` 的复现命令选择 release benchmark，单独运行，避免与编译或负载测试竞争资源。
- DeepSWE 适配器改动读取 `benchmarks/deep-swe/README.md`，优先 `pnpm benchmark:deep-swe:test`。适配器单测不运行模型评测，也不证明真实模型任务成功。

报告所验证的源码/产物、实际执行的检查、结果和未覆盖的关键边界。假模型 fixture、单机多节点、真实多设备、公开发布物分别说明。

## 渲染性能门禁

- 渲染性能必须保证，是正式 GUI 发布的交付条件；个人测试设备安装不因此追加独立测试门禁。涉及消息、Markdown、代码、列表、选择、Tooltip、图片或动画时，检查对应热路径，执行能覆盖改动的性能验证；视觉与功能通过不能代替性能通过。
- 修改前保存基线，修改后重建产物，在同一主机、构建配置、窗口尺寸和固定输入下比较。性能测量独立运行，避免同时编译或施加其他负载；区分冷首次进入与缓存预热后的连续、往返滚动。代码高亮样本应包含不同代码块，不能只重复同一段代码掩盖缓存失效。
- 使用当前 `zork-gui-render-bench` 检查确定性回放、可见行上限和 CPU 帧耗时；原生滚动及显示性能用 `scripts/test-desktop-performance.py`。预算以当前脚本为准，不能放宽阈值、减少负载或复用旧二进制来取得通过。
- 超出预算或在同条件复测中出现稳定退化时，继续诊断、修复并重测，性能门禁未通过不得宣称渲染交付完成。报告前后 p95/p99 CPU 耗时、负载、可见行范围和证据路径；实际 FPS 仅在可用显示会话中测量，受刷新率限制，不能从虚拟时钟推算。
- 桌面不可用时继续完成可执行的 headless/CPU 检查，明确原生显示验证尚未完成及原因，不把缺失测量写成通过。
- 原生应用若在 `Application::run` 退出时直接结束进程，报告写入与门禁断言必须在退出之前执行；检查失败退出码，避免窗口已关闭但断言从未运行的假通过。

## 构建目录和磁盘预算的正确用法

- 先检查仓库 `.env` 中的构建配置，遵循 `docs/rust-build-cache.md`。共享脚本/skill 不写私人路径；未配置时用普通本地 `target`。
- 优先使用已读取配置的现有 pnpm 或 Python 入口。直接执行 Cargo 或其他脚本前，在仓库根目录运行 `eval "$(python3 scripts/lib/build_env.py --shell)"`。显式环境变量优先；不要假定 Cargo 自己读取 `.env`。
- 需要隔离时使用配置根目录的 `isolated/<任务名>`，不要为了绕开锁临时另建 `/tmp` 大缓存；没有共享配置时允许正常本地工作流。
- `cache:status` 只预览。回收前停止相关构建和运行任务，期间不启动新任务；检查候选后才运行 `cache:prune`。跳过在用/近期目录不是故障，不要绕过检查强删。
- 预算是软回收阈值，不是 100 GiB 硬配额；kache 命中、缓存预算和 target 占用是不同指标。报告实际 `df`、缓存命中和执行范围，不声称配置即代表验证通过。

- 消息压力测试基准为 **100000 条、所有当前消息呈现类型混排**，分别覆盖用户/助手正文、所有 Markdown 块与行内样式、长内容、结构化批注与文本附件、文件/图片预览及投递/活动状态。须验证起始/中间/末尾区域、字号变化、消息和附件虚拟化；记录真实载入数量、解析数量、可见节点数量与内存。
- 当前桌面原生门禁使用 `python3 scripts/test-desktop-performance.py --messages 100000`；其原生窗口与离屏 `zork-gui-render-bench` 使用同一份 `RootView::render_benchmark_fixture`。旧版兼容界面及其独立性能入口已退役。
- 历史文件按需展开时，分别验证默认聊天态和展开后的文件列表；折叠状态通过不能代替文件列表的压力测试。验证实际滚动、预览、关闭、Esc 和点击外部关闭，保留条目总数及可见/构建范围。
- 离屏全类型压力回放设置 `ZORK_SCROLL_ALL_MESSAGES=1 ZORK_BENCH_MESSAGE_COUNT=100000`；以实际生效的字体配置记录结果；当前正文固定为 13 px，不使用已失效的字号环境变量。先完成编译再运行测量；截图、首次字体/语法准备和持续滚动成本分别记录。

## 测试包收尾

- 创建临时 `.app`、安装器或测试交付包的任务负责收尾：测试结束先退出本任务启动的实例，确认无进程使用，再删除额外临时包和 staging 副本；worktree 固定槽位内的一份当前 app 保留。不要关闭或删除其他任务正在使用的包。
- macOS 临时应用若注册过 Launch Services，收尾时用系统 `lsregister -u <本任务.app路径>` 注销，再删除包；不要重置整台机器的应用数据库。
- 保留必要的测试报告、截图、版本和 SHA-256 即可，除固定 worktree 当前 app 外，不把散装 `.app` 长期留在 artifacts、临时目录或共享归档中，以免出现在 Spotlight/应用列表。
- 用户明确要求保留的发布候选或回滚包使用压缩归档，并记录用途；正式安装的客户端、应用数据和身份不属于临时包清理范围。
- 打包/验收脚本使用 `try/finally` 或 shell trap 清理本任务资源；不得按全局名字通配删除其他任务的输出。

## 独立构建产物必须收尾

- 临时隔离 `CARGO_TARGET_DIR` 由创建任务负责回收。任务完成、所需交付物已保存且无进程使用后，清理本任务独立目录中的编译产物、CEF staging 和临时测试包，不把整套 target 迁往共享盘长期留存。
- 保留少量当前活跃 target 与 kache 复用存储；不能以“可能回滚”或“缓存可以复用”为由给每个任务永久保留一套多 GiB 的依赖/引擎副本。
- 删除边界是本任务的可再生成产物，不包括源码改动、对话、数据库、模拟器数据、测试报告或其他任务正在使用的文件。共享默认 target 不能由单个任务随意清空。
- 收尾报告说明实际删除/保留范围；复制到别处不算释放整体占用，预算配置也不算已经执行回收。

## 每个 worktree 常驻一份最新测试应用

- 使用 `scripts/package-macos-client.py` 更新 `.tmp/macos-app.noindex/Zork.app`。不为每轮测试新建带时间戳的包目录；不同 worktree 独立，同一个 worktree 的更新由锁串行化。
- 新包先构建/验证，成功后才退出该槽位的旧实例并替换；原来在运行则重启新版。构建失败保留旧版。不要用 bundle ID 广泛退出别的 worktree 或正式安装客户端。
- 默认保留当前 `.app`，不生成压缩包、不保留历史 app。事务中允许短暂新旧两份，完成后清理暂存和旧副本。此固定当前 app 是临时测试包清理规则的例外。
- `--launch` 用于交互运行；`--run <测试命令>` 的 `{app}` 指向替换后的固定 app，测试结束清理所启动的进程但保留当前 app。
- 只有明确交付/导出时才传 `--output` 生成压缩包。`--keep-app` 已移除，因为保留当前 app 现在是默认行为。
- 独立浏览器打包 CLI 使用同一 worktree 槽位/锁，不能再留另一份长期 app；CEF helpers 是主包内部组件。
