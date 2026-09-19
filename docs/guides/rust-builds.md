# Rust 构建缓存约束

## 目标

workspace 内直接依赖同一个第三方 crate 时，使用同一版本和同一组 feature，避免单包和多包构建仅因 member 的声明不同而生成额外编译变体。

## 约束

- feature 曾经存在分叉的公共依赖在根 `Cargo.toml` 的 `[workspace.dependencies]` 中声明一次，取 workspace 当前实际需要的 feature 并集。
- member 通过 `dependency.workspace = true` 继承，不再声明自己的版本、默认 feature 或 feature 子集。
- 不添加仅用于影响 feature 解析的依赖或 crate。
- 不把 debug/release、目标平台、编译器版本、源代码和 `rustflags` 不同的产物视为同一缓存项。

统一依赖的实际范围以根 `Cargo.toml` 为准。`getrandom` 的 `wasm_js` 后端仅在 Web crate 的 wasm 目标依赖中启用。

## 边界

稳定 Cargo 根据一次命令实际选中的包统一 feature。`[workspace.dependencies]` 统一直接依赖声明，但不会强制没有直接使用某个依赖的 member 激活该依赖，也不会改写传递依赖的 feature。

## 验证

用 `cargo metadata --locked --format-version 1` 检查 workspace 声明与 member 依赖；对实际构建的包运行 `cargo tree --locked -e features -p PACKAGE`，核对 feature 来源。当前没有独立的 workspace feature 一致性测试，不能将这项约束视为已由专用测试自动保证。

## macOS Metal 工具链

原生客户端在构建时编译 Metal shader，避免每次启动编译源码。macOS 构建机需要与 Xcode 匹配的 Metal Toolchain；构建前用 `xcrun --find metal` 和 `xcrun --find metallib` 检查。修改 shader 或生成绑定后重建原生渲染器，再验证实际绘制结果。

GPUI Web 使用当前 Rust 的 WebAssembly target，或匹配该编译器的 `rust-src`。
若工具链没有自带 linker，可在忽略的 `.env` 中用 `ZORK_WASM_LD` 指定兼容的
`wasm-ld`；未指定时优先使用 PATH 中的 `wasm-ld`，否则保留 Cargo 的默认选择。
显式 `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER` 优先，不依赖固定 Homebrew
Cellar 版本，也不修改全局 LLVM 链接。

## 可选的本机存储配置

真实机器路径放在被 Git 忽略的 `.env`，不写进共享脚本或 skill：

```dotenv
ZORK_BUILD_ROOT=/path/to/dedicated-build-cache
ZORK_BUILD_BUDGET_GIB=100
ZORK_BUILD_LOW_WATER_GIB=80
# 仅网络盘需要；未挂载则拒绝运行。
# ZORK_BUILD_MOUNT=/path/to/mount
```

`ZORK_BUILD_ROOT` 下默认 Cargo 输出为 `target`，Android 独立入口为
`android`；手工隔离构建可放 `isolated/<任务名>`。不配置时仍使用项目
`target`，不会要求共享盘。路径相对仓库根目录解析，支持引号和 `~`，
不执行命令、也不展开 `$变量`。进程环境变量优先于 `.env`；显式
`CARGO_TARGET_DIR` 优先于自动生成的路径。

更换开发机时，在新主机创建本地构建根并重新构建。需要兼容直接读取 `target`
的入口时，让该链接指向同一构建根的 `target` 子目录，不沿用旧主机的绝对路径。
源码、锁文件和必要资产随工作区迁移；节点运行数据、身份与活跃数据库按独立的
运行环境管理，不随构建缓存切换。

现有 pnpm 的 Rust build/dev/test/start 入口，以及 Android、storybook、
桌面 headless Python 入口读取配置。其他脚本或直接 Cargo 命令不会自动
读取 `.env`，在仓库根目录先执行：

```sh
eval "$(python3 scripts/lib/build_env.py --shell)"
cargo test --locked -p zork-config
```

以上只输出允许的构建变量，不会加载产品密钥。切换仓库或修改 `.env`
后建议开新 shell；已经 export 的值会优先，必要时先 unset 再加载。

### 预算与显式回收

```sh
pnpm cache:status          # 只预览
pnpm cache:prune           # 显式执行回收
```

预算统计配置根目录的磁盘占用。超出高水位后，按最后修改时间选择旧
`target`、`android` 和 `isolated/*` 中带 Rust 缓存标识的目录，目标降到
低水位。默认保留最近 24 小时修改的目录；不遍历任意源码目录，不跟随
候选目录软链接。根目录内的其他文件会计入占用，但不会自动删除。

**执行回收前停止这个缓存根目录的构建和运行任务，并在清理结束前不要
启动新任务。** 脚本再次检查文件修改时间和打开的文件，检查失败或目录
在使用中则拒绝/跳过；这些检查无法对未协作的新进程提供原子互斥。
随后调用 `cargo clean --target-dir`，不直接删除源码或数据库。
共享目录需由同一主机专用；本机无法确认其他主机的打开文件。

这是手动软预算，不是文件系统硬配额；近期或在用产物可能使预算无法
达到。kache 的本地存储预算、共享远端容量与这里的 target 预算互相独立。

## 可选 CI 远端缓存

CI 可通过仓库变量 `KACHE_S3_ENDPOINT`、可选 `KACHE_S3_BUCKET` 及对应的 `KACHE_S3_ACCESS_KEY` / `KACHE_S3_SECRET_KEY` secrets 接入 R2 缓存。未配置时普通构建仍可运行，容器构建使用同一显式配置。账号端点和秘密不写入共享源码。

依赖或工具链变化、缓存丢失后，可手工引导运行时缓存：
`gh workflow run ci.yml --ref main -f prime_cache=true`。这条路径只构建并保存库、程序和测试产物，不执行运行时契约，不能算作 CI 验证通过；完成后仍运行普通 main 检查。main 的 GitHub target 缓存须在 main 上引导，分支缓存不能反向供 main 使用。R2 按构建命名空间共享，可在受信任分支引导相同构建输入，之后仍验证 main。

即使启用 kache，CI 也在构建成功后、测试之前保存完整 Cargo target；两者分别复用编译产物与 Cargo 指纹/本地构建脚本输出。target 键包含锁文件与源码提交，并可回退到兼容的旧版本，避免不可变缓存键永远保留旧源码产物。手动 workflow dispatch 可在指定分支做预热或完整验证；分支缓存仍不能供 main 使用。

CI 在同一次 Cargo target 选择中构建运行程序和测试，避免两次命令切换开发依赖 feature 后反复生成同一依赖的不同变体；后续测试仍校验当前源码指纹。

target 同时保存受版本控制的输入文件哈希、权限与构建时的修改时间。新 checkout 只对内容及权限完全相同的普通文件恢复原时间，避免 checkout 时间导致 Cargo 重建全部路径依赖；变更文件、符号链接和缺失记录保留当前状态，继续由 Cargo 判断。不能统一回写提交时间或无条件信任缓存中的路径。
