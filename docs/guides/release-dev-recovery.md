# release/dev/test 开发与自救

三个通道的来源固定：**dev 是 main 分支的构建**，功能分支合并进 main 之后才构建和安装 dev，不从功能分支或集成分支直接打 dev；**release 是稳定的 dev 版本**，只由通过观察期的 dev 候选提升而来，同时保留为独立救援入口；Agent 开发、功能分支和回归使用 test。日常使用 dev。三套客户端分别使用自己的名称、
bundle 身份、数据、偏好、节点身份和 Mesh；不要用复制整份 release 数据的方法
初始化 dev，也不要让不同通道加入同一个日常 Mesh。

这是本机开发候选的事务流程。公开的四平台节点发行另见
[原生发布](native-releases.md)。`zork update` / `zork upgrade` 不替代这里的完整
数据快照和恢复验收。

## Agent 测试与日常 dev 的边界

工作树打包、候选构建和 Android 开发构建默认选择 test。工作树 macOS App
使用独立的签名身份与实例数据，Finder 启动也不会打开日常 dev；`pnpm dev`
与 `pnpm dev:station` 使用该 checkout 的 `.tmp/test-node`。测试只停止自身路径
下的进程，不按共享进程名或 bundle ID 退出客户端。并行任务使用独立工作树，
Android 使用独立模拟器；测试结束按验证规则回收自己的数据与实例。

`--profile dev/release` 只决定 Cargo 优化配置，与产品通道无关。构建测试候选：

```sh
python3 scripts/dev/recovery.py --root /path/to/task-deployment install-tools --repo "$PWD"
python3 scripts/dev/recovery.py --root /path/to/task-deployment build --repo "$PWD" --kind app
```

用户要求更新日常 dev 时，显式将已验证 test 候选转为 dev 身份，再走原有安装事务：

```sh
python3 scripts/dev/recovery.py --root /path/to/deployment accept-test /path/to/test-candidate --repo "$PWD"
```

此步骤复用代码和构建记录，不复制测试数据；加 `--switch` 才切换该部署根下的 dev。
远端安装指定 `update-macos-client.py --channel dev --candidate /path/to/dev-candidate`。
直接构建日常 dev 也必须显式 `--channel dev`，并且只从 main 的提交构建；Android 使用
`scripts/android/build.py --channel dev`。Android 应用 ID 按 release（无后缀）、
`.dev`、`.test` 对齐。
release 的观察期与提升要求保持不变。旧部署配置再次 `install-tools` 时只补缺少的
通道，不覆盖既有配置、身份或健康检查设置。

## 建立独立入口

在构建主机的任务 checkout 中安装可独立运行的管理脚本：

```sh
python3 scripts/dev/recovery.py --root "$HOME/Zork" install-tools --repo "$PWD"
```

安装器生成本机 `deployment-config.json`。**已有安装先把各通道的 data、payload
指向原目录**，不要迁移或清空身份。给每个受管节点配置专用健康检查使用的
Profile ID、model 和 thinking；凭据仍由该节点自己的 Profile 保存。默认生成的
目录适用于新环境，不能据此认定旧的 release 已被接管。

管理脚本和恢复日志保存在应用、二进制和节点数据之外。更新脚本不启动、不停止
节点。两个通道的存储路径必须互不包含；数据中的外部符号链接会使快照拒绝执行，
需要先明确该外部存储的归属，不能忽略错误继续切换。

## 构建和切换节点

```sh
~/Zork/bin/zork-dev-build /path/to/checkout
~/Zork/bin/zork-dev-build /path/to/checkout --switch
~/Zork/bin/zork-node dev status
~/Zork/bin/zork-node dev health
~/Zork/bin/zork-node release status
```

不带 `--switch` 只生成不可变候选。候选记录源码 commit、未提交内容的摘要、
工具链、构建配置以及完整产物摘要。构建从 Cargo 报告的可执行文件即时取样，
不在构建结束后从共享 target 猜选二进制。源码在构建期间变化或任一组件缺失，
构建失败，运行版本保持原样。

部署构建使用配置构建根下固定的 `isolated/deployment`，从编译到取样、打包持有
同一把锁；普通 Cargo、bench 和其他 worktree 不使用这个目录。这样既保留
kache 复用，也防止 Cargo 退出后其他构建替换同名产物。它是后续部署继续使用的
固定缓存，不按构建次数复制 target；没有配置构建根时位于 checkout 的 target 下。

切换会暂停目标实例的服务看护、停止写者，复制并校验完整数据与旧包。
配置的外置 preferences 也自动纳入快照和通道重叠检查。
所有快照成功后才迁移配置和替换包。启动验收同时检查：

- 运行 PID 对应的可执行路径、文件 inode 和 SHA-256 与候选一致；
- supervisor、Station、内嵌 Agent 的数据根、协议和就绪状态一致；
- 原 Mesh 身份及已有聊天的近期消息锚点仍然可读；
- 在新建的健康检查 Chat 中，用户输入入源，Agent 调用 `chat.post_message`，
  带本轮随机标记的回复回到同一 Chat。

健康检查会调用配置的模型。进程存在、端口开放或 `running:true` 都不能产生成功
观察记录。历史锚点是在线抽检；完整快照负责恢复全部业务库、原始消息源、附件、
身份与配置，不能仅备份 SQLite 或二进制。

## 更新 Mac 客户端

构建与安装分别由公共打包入口和指定目标的更新入口负责。目标必须显式提供：

```sh
python3 scripts/update-macos-client.py --host "$DESTINATION_MAC" \
  --channel dev --profile "$HEALTH_PROFILE" --model "$HEALTH_MODEL" --thinking off
```

默认更新 test；更新日常 dev 显式指定 `--channel dev`。打包时就写入通道和完整构建记录，再逐层签名；不在签名后修改
plist。dev 的显示名为 `Zork Dev`，身份为 `ing.zork-dev.desktop`，默认数据为
`~/Zork/client-dev`；release 保持 `Zork`、`ing.zork.desktop` 和原有
`~/Library/Application Support/Zork/client`。通道标记也对直接启动 GUI 可执行
文件生效，不依赖 Launch Services 注入机器绝对路径。

已验证的候选使用 `--candidate DIR --expected-build-id ID` 安装。ID 绑定整个
bundle 及其 helpers，不只检查 GUI。安装器只关闭目标 app 的实际进程，不按
共享名称或 bundle ID 广泛退出应用。该客户端原本启用的本机节点必须完成聊天
验收；未配置健康 Profile 或未启用本机节点时，不把仅 GUI 存活当作完成。

私人主机的默认 SSH 目标仍由本地更新入口提供，不写入共享配置或 Git。

## 一周观察与提升

每天在实际使用的同一个 dev 产物上运行一次 `health`。Mac 安装后的远端检查：

```sh
python3 scripts/update-macos-client.py --host "$DESTINATION_MAC" --observe \
  --profile "$HEALTH_PROFILE" --model "$HEALTH_MODEL" --thinking off
```

观察记录绑定完整候选 ID。两次成功检查之间最多允许 36 小时，最后一次必须在
24 小时以内；首末成功检查至少相隔七天。重新部署（包括相同候选）、版本改变、失败记录或观察中断重新
开始累计。记录表达每日检查和使用反馈所证明的稳定期，不宣称连续监测每一秒；
发现实际故障时运行失败的健康检查并保留记录。没有修改时间或绕过观察期的参数。

节点提升：

```sh
~/Zork/bin/zork-promote /path/to/checkout
~/Zork/bin/zork-promote /path/to/checkout --yes
```

Mac 提升：

```sh
python3 scripts/update-macos-client.py --host "$DESTINATION_MAC" --promote \
  --profile "$HEALTH_PROFILE" --model "$HEALTH_MODEL" --thinking off
# 同一命令加 --switch 才安装 release。
```

提升前再次核对实际运行的 dev 并完成聊天。提升复用已经观察的代码和依赖，
客户端只重新绑定 release 的 bundle 身份和签名，随后再次完整验收。
不会在提升时偷偷重编译为另一种优化配置；需要优化构建时，最初就用
`zork-dev-build --profile release` 开始这份候选的观察期。

## 故障恢复

任何备份、替换、启动或健康检查失败都返回非零。恢复把旧包、完整数据和原服务
状态一起恢复；失败版本写入的数据单独保存在事务的 `failed-*` 目录中，不混入
旧 schema，也不自动重放它可能已经执行的外部操作。

部署进程被强制终止或恢复再次失败时，后续修改会被 pending 记录阻止。
从 release 的 Agent、SSH 或本机终端执行 journal 对应的恢复入口：

```sh
python3 /path/to/installed/tools/dev/recovery.py --root "$HOME/Zork" \
  recover /path/to/transaction
```

安装回执保存精确恢复命令；原始入口也可以执行相同的 `recover` 子命令。
恢复前重新校验快照，损坏快照不会覆盖现有数据。恢复后保留 Mesh 身份与原消息
ID，并为恢复的 Chat 源和同步投影建立新 epoch，使旧客户端游标失效。
成功和失败快照都需要保留到其回滚、调查用途结束，再由操作者明确清理；不按
时间或目录大小自动删除。

## 隔离演练

先重建本次候选，再运行：

```sh
python3 scripts/test-deployment-recovery.py
python3 scripts/test-release-dev-recovery.py --candidate /path/to/candidate
```

第二项运行真实 supervisor、Station 和 Agent，以独立数据、身份、端口和外部
HTTP fixture 模型演练构建失败、备份失败、启动失败、换包失败及部署进程突然
退出；独立 release Agent 通过真实 shell 工具救回 dev。可用 `--live-profile`、
`--live-model` 和 `--live-thinking` 增加真实供应商的隔离聊天与配置重启验收。

三通道 GUI 与安装事务使用 `scripts/test-macos-channel-recovery.py --candidate /path/to/app-candidate --output /path/to/report`，
从校验过的 Cargo 输入用当前打包器生成两份独立身份，核对内嵌 build_record 后运行正式安装器。
也可提供同一构建的 `--dev-app` / `--release-app` / `--test-app`；只接受
`ing.zork.recovery-fixture.*` 的独立签名包，拒绝拿用户的正式 bundle 身份演练。
桌面实际输入到客户端收到回复另用 `scripts/test-desktop-startup.py`。
按[验证 skill](../../.agents/skills/zork-validation/SKILL.md)运行已批准关键门禁，
将功能恢复、真实模型、目标设备与性能的结果分别保存在本地 ignored 产物中。
