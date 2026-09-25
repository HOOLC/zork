# Rust 构建缓存约束

## 目标

workspace 内直接依赖同一个第三方 crate 时，使用同一版本和同一组 feature，避免单包和多包构建仅因 member 的声明不同而生成额外编译变体。

## 约束

- feature 曾经存在分叉的公共依赖在根 `Cargo.toml` 的 `[workspace.dependencies]` 中声明一次，取 workspace 当前实际需要的 feature 并集。
- member 通过 `dependency.workspace = true` 继承，不再声明自己的版本、默认 feature 或 feature 子集。
- 不添加仅用于影响 feature 解析的依赖或 crate。
- 不把 debug/release、目标平台、编译器版本、源代码和 `rustflags` 不同的产物视为同一缓存项。

统一依赖的实际范围以根 `Cargo.toml` 为准。

## 边界

稳定 Cargo 根据一次命令实际选中的包统一 feature。`[workspace.dependencies]` 统一直接依赖声明，但不会强制没有直接使用某个依赖的 member 激活该依赖，也不会改写传递依赖的 feature。

## 验证

用 `cargo metadata --locked --format-version 1` 检查 workspace 声明与 member 依赖；对实际构建的包运行 `cargo tree --locked -e features -p PACKAGE`，核对 feature 来源。当前没有独立的 workspace feature 一致性测试，不能将这项约束视为已由专用测试自动保证。

## macOS Metal 工具链

原生客户端在构建时编译 Metal shader，避免每次启动编译源码。macOS 构建机需要与 Xcode 匹配的 Metal Toolchain；构建前用 `xcrun --find metal` 和 `xcrun --find metallib` 检查。修改 shader 或生成绑定后重建原生渲染器，再验证实际绘制结果。

## 可选的本机存储配置

macOS App 打包默认包含原生 cua 宿主。首次打包按仓库固定的上游修订和锁文件构建驱动，
后续复用经过摘要校验的运行产物；纯节点构建不需要桌面驱动。已有 cua checkout 和独立
Cargo 缓存可在本机 `.env` 中配置 `ZORK_CUA_SOURCE`、`ZORK_CUA_TARGET_DIR`，避免重复下载和编译。
源码必须保持固定修订且无本地修改；固定修订由 `scripts/lib/cua-runtime.py` 维护。
也可用打包入口的 `--cua-runtime` 指向经过验证的运行产物。自救候选同时记录此输入、原始许可证
及整包摘要，不能只记录 Station/GUI 的 Cargo 可执行文件。

真实机器路径放在被 Git 忽略的 `.env`，不写进共享脚本或 skill：

```dotenv
ZORK_BUILD_ROOT=/path/to/dedicated-build-cache
ZORK_BUILD_BUDGET_GIB=100
ZORK_BUILD_LOW_WATER_GIB=80
# 仅网络盘需要；未挂载则拒绝运行。
# ZORK_BUILD_MOUNT=/path/to/mount
```

`ZORK_BUILD_ROOT` 下主 checkout 的默认 Cargo 输出为 `target`，Git worktree
默认使用 `isolated/<worktree 名>`，避免不同源码修订共用 Cargo 指纹和库产物；
Android 独立入口为 `android`。不配置构建根时仍使用各 checkout 自己的
`target`，不会要求共享盘。路径相对仓库根目录解析，支持引号和 `~`，
不执行命令、也不展开 `$变量`。进程环境变量优先于 `.env`；显式
`CARGO_TARGET_DIR` 优先于自动生成的路径。

更换开发机时，在新主机创建本地构建根并重新构建。需要兼容直接读取 `target`
的入口时，让该链接指向同一构建根的 `target` 子目录，不沿用旧主机的绝对路径。
源码、锁文件和必要资产随工作区迁移；节点运行数据、身份与活跃数据库按独立的
运行环境管理，不随构建缓存切换。

现有 pnpm 的 Rust build/dev/test/start 入口，以及 Android、zork-design-pc、
桌面 headless Python 入口读取配置。其他脚本或直接 Cargo 命令不会自动
读取 `.env`，在仓库根目录先执行：

```sh
eval "$(python3 scripts/lib/build_env.py --shell)"
cargo test --locked -p zork-config
```

以上只输出允许的构建变量，不会加载产品密钥。切换仓库或修改 `.env`
后建议开新 shell；已经 export 的值会优先，必要时先 unset 再加载。

构建封装清除 hook 继承的 Git 仓库定位、索引和临时配置变量，避免依赖构建
中的 Git 命令误改父仓库；SSH 与凭据辅助程序等传输设置保留。直接调用
Cargo 前同样使用上面的 `--shell` 入口，使清理作用于后续子进程。

### 预算与显式回收

```sh
pnpm cache:status          # 只预览
pnpm cache:prune           # 显式执行回收
python3 scripts/build/cache_budget.py --reclaim-released --dry-run  # 预览已结束工作树
pnpm cache:reclaim-completed  # 回收已结束工作树的隔离 target
python3 scripts/build/cache_budget.py --auto --dry-run  # 预览自动策略
python3 scripts/build/cache_budget.py --auto            # 供本机维护任务定期执行
```

预算统计配置根目录的磁盘占用。超出高水位后，按最后修改时间选择旧
`target`、`android` 和 `isolated/*` 中带 Rust 缓存标识的目录，兼容旧的
`isolated/*/target` 布局，目标降到
低水位。默认保留最近 24 小时修改的目录；不遍历任意源码目录，不跟随
候选目录软链接。在目标目录放 `.zork-cache-keep` 可显式保留需复查的构建缓存，
用完后由保留者移除该标记。根目录内的其他文件会计入占用，但不会自动删除。

`--auto` 只回收已释放的隔离 Cargo target，不清理共用的 `target`、
Android 产物或源码；适合在个人开发机定期运行。通过 `build_env.py` 使用
`isolated/<worktree 名>`（或其 `target` 子目录）时会登记来源 worktree；
入口新建目标目录时也会写入有效的 Cargo 缓存标记，已有无标记目录不会被
自动补标。显式设置 `CARGO_TARGET_DIR` 要先于调用 `build_env.py`。只有
该 worktree 已移除、目标超过保留期且没有打开文件，才允许自动回收。这避免
定期任务与仍在进行的构建之间仅凭一次占用检查作决定。固定用途的隔离目录与
未登记的旧目录仍由其任务所有者管理。自动任务串行化回收实例，并在每个目标
回收前重新检查来源、Cargo 的 `CACHEDIR.TAG`、修改时间与打开文件。若这些
目标使占用仍高于预算，下次运行继续检查。旧目录缺少 `CACHEDIR.TAG` 时需
人工核对其内容，不能由自动任务补造标记。`--apply` 保留手动
回收共用目标的能力。

任务结束并移除工作树后，运行 `cache:reclaim-completed` 立即回收其隔离 target；
此入口不受预算和 24 小时保留期约束，仍检查来源、缓存标记、保留标记和打开文件。
实际清理会在构建根的忽略日志中记录目标、前后占用及磁盘可用量。
构建封装以 `build_env.py -- <command>` 执行时，在构建根的忽略日志中记录
命令耗时与构建前后磁盘可用量，不记录命令参数。并行构建期间的磁盘变化属于
整个卷；比较某次清理的回收量和重建代价时，先让其他构建结束，再用同一命令
单独测量。`--shell` 后直接运行的命令不在这份记录中。

手动清理共用 `target` 或 `android` 前，停止使用该目标的构建与运行任务。
自动模式只处理独立目标；脚本检查修改时间和打开文件，检查失败或目录在用
则拒绝/跳过。未通过仓库构建入口启动的进程仍需遵守目标目录的占用边界。
随后调用 `cargo clean --target-dir`，不直接删除源码或数据库。
共享目录需由同一主机专用；本机无法确认其他主机的打开文件。

### 按周整理构建存储

```sh
python3 scripts/prune-build-storage.py            # 预览
python3 scripts/prune-build-storage.py --apply    # 执行
python3 scripts/prune-build-storage.py --install-schedule   # 安装每周 launchd 任务（主 checkout）
```

删除来源 worktree 已不存在（登记标记，否则按 `isolated/<worktree 名>` 命名推断）或
`--days`（默认 14）天未写入的隔离 target，共用 `target` 只按天数；`isolated/deployment`
与带 `.zork-cache-keep` 的目录保留，删除前复查 `CACHEDIR.TAG`、一小时内写入和打开文件。
同时删除本仓库各 worktree 中被忽略且 7 天未改动的 `artifacts/` 条目（含已跟踪文件的
条目保留），执行 `git worktree prune`，并运行 `kache gc` 直到本地 kache 存储回到配置
上限以内；输出每项原因和回收量。只处理 Zork 的 worktree、构建根和 kache 存储。

这是手动软预算，不是文件系统硬配额；近期或在用产物可能使预算无法
达到。kache 的本地存储预算、共享远端容量与这里的 target 预算互相独立。

## 可选 CI 远端缓存

CI 可通过仓库变量 `KACHE_S3_ENDPOINT`、可选 `KACHE_S3_BUCKET` 及对应的 `KACHE_S3_ACCESS_KEY` / `KACHE_S3_SECRET_KEY` secrets 接入 R2 缓存。未配置时普通构建仍可运行，容器构建使用同一显式配置。账号端点和秘密不写入共享源码。

依赖或工具链变化、缓存丢失后，可手工引导运行时缓存：
`gh workflow run ci.yml --ref main -f prime_cache=true`。这条路径只构建并保存库、程序和测试产物，不执行运行时契约，不能算作 CI 验证通过；完成后仍运行普通 main 检查。main 的 GitHub target 缓存须在 main 上引导，分支缓存不能反向供 main 使用。R2 按构建命名空间共享，可在受信任分支引导相同构建输入，之后仍验证 main。

即使启用 kache，CI 也在构建成功后、测试之前保存完整 Cargo target；两者分别复用编译产物与 Cargo 指纹/本地构建脚本输出。target 键包含锁文件与源码提交，并可回退到兼容的旧版本，避免不可变缓存键永远保留旧源码产物。手动 workflow dispatch 可在指定分支做预热或完整验证；分支缓存仍不能供 main 使用。

CI 在同一次 Cargo target 选择中构建运行程序和测试，避免两次命令切换开发依赖 feature 后反复生成同一依赖的不同变体；后续测试仍校验当前源码指纹。

target 同时保存受版本控制的输入文件哈希、权限与构建时的修改时间。新 checkout 只对内容及权限完全相同的普通文件恢复原时间，避免 checkout 时间导致 Cargo 重建全部路径依赖；变更文件、符号链接和缺失记录保留当前状态，继续由 Cargo 判断。不能统一回写提交时间或无条件信任缓存中的路径。
