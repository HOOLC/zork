# Zork Android

第一版移动访问端，Android 10 / API 29 以上，arm64-v8a。Kotlin / Compose
界面通过 JNI 调用 Rust 客户端核心；Synch 0.1.8 直接嵌入应用进程，不依赖
CLI、后台子进程或本地 gRPC 服务。共享服务页面按需建立 loopback HTTP 入口。任务继续在原 Station / Agent
设备上执行。

Compose 负责原生输入、呈现和系统生命周期，协议、业务规则及持久状态复用
[Rust core 合同](../../docs/design/client-core.md)；当前工程和验证入口已取代早期的技术选型/连接实验计划。

<a id="mesh-adb"></a>

## 通过 Mesh 安装与调试

在手机的「设置 → 安卓调试」开始设置。页面按实际探测结果只显示当前需要处理的步骤，命令和端口编辑在 USB 激活说明中。手机就绪后，每台已接入的 Station 都可以同时连接，各台的授权、重试和连接状态分别显示；开启期间由前台服务保持后台连接。Station 需要 Android SDK Platform-Tools；Agent 使用内置 `android-debugging` Skill 与 `android.devices` 查询该 Station 上的手机和状态。

蜂窝网络调试需要先用 USB 激活页面显示的 ADB 网络端口，激活后可以拔线；手机重启后需再次激活。首次系统授权仍在手机确认。开发 Zork 时保留稳定版承载 Mesh，安装独立 `.debug` 包名的开发版。边界与连接所有权见 [设备合同](../../docs/design/devices.md#android-debugging)。

协议回归可在重建 Station 后运行 `zork-client-core` 的 ignored `adb_mesh` 集成测试，通过 `ZORK_TEST_STATION_BIN` 与 `ZORK_TEST_ADB_BIN` 指定本次构建和 SDK ADB。该测试隔离 ADB server 并模拟 adbd；手机系统授权、蜂窝跨网和实际 APK 安装仍需真机验收。

## 设计真值

本应用按 `apps/zork-design-pc/mobile/design-spec.md` 与
`mobile/prototype/index.html` 的 **nav7** 导航结构，并应用 [当前界面基线](design.md) 中的设置与控件修订；早期参考截图在
`artifacts/android/nav7/reference`，原生对照在 `artifacts/android/nav7/comparison`。
桌面 `gui.html` 和最初的三屏概念图均不替代当前移动端规范。

导航 48dp、6dp 圆角、2dp 间隔、整行按压；列表无持续选中态。
底部两个等宽入口，导航和会话各占一屏。成员头像、文件卡片、片段评论与
自动增高输入框使用原有 SVG 路径的 Android VectorDrawable 转换结果。
转换脚本为 `scripts/android/export_design_assets.py`，没有重绘品牌资产。

## 当前能力

- 扫码或粘贴桌面邀请，桌面确认后自动连接。
- 查看领队和已有任务，打开对话；任务直接显示，不再提供独立展开箭头，长按查看详情。
- Composer 与桌面采用同款暖白液态轮廓、细描边与伙伴活动胶囊；最多三行，更多内容在输入区内部滚动。伙伴展开时消息同步避让，浏览历史保持位置。
- 查看分页消息与实时活动，发送消息和请求停止。用户消息按原文显示，助手回复支持 Markdown；两者都保留文字选择和片段评论。
- 点击顶部成员头像或输入框上方的成员活动胶囊查看该成员的 Session 执行历史；支持常规操作分组、时间轴缩放与选区、对象跳转、翻页、重试、用量／额度概览及完整记录复制、原始 JSON 详情。执行历史只由 Rust core 按需读入内存，与聊天消息的持久缓存分开。
- 系统文字选择中的“评论”可累积、编辑、移除，和正文一次发送。
- 支持 UTF-8 文本附件（单个 24 KiB、最多 4 个，整条仍受 core 64 KiB 限制），
  可预览/保存；草稿和投递使用与桌面相同的兼容消息格式。
- 聊天 → 设备 → 队员／大模型 → 原对话保留草稿与阅读位置。设备卡片支持改名并同步，仅展示手机能操作的设置；不支持的运行偏好和版本更新入口直接隐藏。
- 队员支持创建、头像与模型编辑；大模型支持订阅授权、API Key 连接、模型获取及手动编辑。凭据只发送到所属设备，不写入手机状态快照。
- 已读取的历史、每个对话的草稿和待发送队列保存在应用私有目录。
- 消息入队与清空草稿使用和桌面端相同的 SQLite 事务；重试沿用原请求 ID。
- 未开始投递的消息可原子地退回草稿，保留后来输入的内容；已尝试投递的消息
  保留到收到回执，不伪称已撤回。
- 本地草稿、缓存和消息入队使用独立通路，不等待慢网络请求。
- 设置提供工具连接、队员技能与设备服务的只读详情；手机通过扫码/粘贴加入，邀请生成和审批留在电脑端，详见 [设置范围](../../docs/design/interface.md#mobile)。
- 通知复用 core 的事件分类、去重、隐私和免打扰规则。用户可开启后台保持连接，由 Android 远程消息前台服务承载；未开启时离开前台会暂停连接。
- 重新打开时恢复身份、订阅、历史和未完成设置操作。授权只保存可恢复的公开元信息，API Key 和粘贴的回调不落盘。系统强制结束进程后，已提交的草稿与队列仍可恢复；关闭客户端不停止远端任务。

初次接入推荐「桌面连接设备 → 连接手机 → 手机扫一扫 → 桌面允许连接」。
成员邀请和审批由 Station 管理；局域网和跨网络中继连接均无需云端账号。可从连接页或「设置 → Zork 账号」按需登录云端服务。手机会自动保存获准
的设备；无需手工交换身份或填写 IP。详见 [手机接入协议](../../docs/design/devices.md#invitations)。

已在 Android 16 模拟器和 OPPO Find N6 上验证二维码图片解码、授权、读取
Station 与重连。镜头光学扫码、蜂窝/Wi-Fi 切换和公网 relay 强制中继的
验证边界见协议文档。独立云推送、任务验收界面、二进制文件
上传及手机执行 Agent 不在此版。
默认中文，沿用仓库的 Zork 标志、Inter 字体和色板。

## 构建

在仓库根目录运行。需要 JDK 17、Android SDK / NDK 和 Rust；依赖版本：

| 组件                      | 固定版本                                           |
| ------------------------- | -------------------------------------------------- |
| Gradle                    | 9.3.1，wrapper 校验 SHA-256                        |
| Android Gradle Plugin     | 9.0.1                                              |
| Kotlin / Compose compiler | 2.2.10                                             |
| compileSdk / targetSdk    | 36                                                 |
| Build Tools               | 36.0.0                                             |
| NDK                       | 28.2.13676358                                      |
| Synch                     | 0.1.8 / `6d6283f09c32476dc77c09f76a2b2529a42a558d` |

SDK 包：`platform-tools`、`platforms;android-36`、`build-tools;36.0.0`、
`ndk;28.2.13676358`。模拟器验证另外需要 `emulator` 和
`system-images;android-36;google_apis;arm64-v8a`。

```sh
python3 scripts/android/build.py
```

日常真机性能体验与采样使用 `python3 scripts/android/build.py --profile`：生成
不可调试且允许 shell 性能采样的 APK，沿用 debug 包名和签名，
可覆盖安装并保留已有数据。默认命令仍生成可调试版本；不要用它判断滚动性能。
`--profile --tests` 可同时构建对应的 instrumentation APK。

普通个人测试设备安装只构建主 APK，不自动执行独立测试。只有明确需要 instrumentation 测试时添加 `--tests` 来构建测试 APK。

Profile 包同时携带应用 UI 的 Baseline Profile（`app/src/main/baseline-prof.txt`）与 Compose 库配置。
通过 ADB 侧载后，在首次交互前安装并编译这个配置；仅安装 APK 往往仍处于 `verify` 状态，
首轮状态切换会混入解释执行和 JIT 开销。以下命令只作用于指定设备上的 Zork 包，保留应用数据：

```sh
adb -s SERIAL install -r apps/android/app/build/outputs/apk/debug/app-debug.apk
adb -s SERIAL shell am broadcast -a androidx.profileinstaller.action.INSTALL_PROFILE -n ing.zork.android.debug/androidx.profileinstaller.ProfileInstallReceiver
adb -s SERIAL shell am force-stop ing.zork.android.debug
adb -s SERIAL shell cmd package compile -f -m speed-profile ing.zork.android.debug
adb -s SERIAL shell am start -W -n ing.zork.android.debug/ing.zork.android.MainActivity
```

安装配置的广播应返回 `result=1`；用 `dumpsys package dexopt` 确认该包为 `speed-profile`。
性能前后对比使用相同编译状态，并记录实际渲染刷新率；单帧端到端延迟、CPU 工作耗时与呈现间隔分别报告。

输出：

- `apps/android/app/build/outputs/apk/debug/app-debug.apk`
- `apps/android/app/build/outputs/apk/androidTest/debug/app-debug-androidTest.apk`（仅 `--tests`）

脚本使用 `Cargo.lock` 和 Gradle 严格依赖锁；修改依赖后需要有意更新锁文件。
JDK / SDK 可由 `JAVA_HOME`、`ANDROID_HOME` 指定。macOS 默认复用 Homebrew
JDK 17 和 `~/Library/Android/sdk`。正常 Rust 工具链可安装
`aarch64-linux-android` target；Homebrew Rust 没有该标准库时，脚本使用与
编译器匹配的 `rust-src` 构建标准库，不替换系统 Rust。

Android 编译缓存默认位于 `target/android`，与桌面 Cargo 构建锁分离；配置
`ZORK_BUILD_ROOT` 时位于该根目录的 `android` 子目录。活跃缓存使用本机磁盘，
路径和回收规则见 [构建指南](../../docs/guides/rust-builds.md)。
保持 `CARGO_INCREMENTAL=0`、`CARGO_PROFILE_DEV_DEBUG=0`、4 个 Cargo 构建任务。
`--skip-native` 仅供确认 Rust 部分没有变动的 UI 迭代；`--native-only` 只构建和
准备 native 资源。不要在 native 代码变更后用旧 `.so` 交付。

更换开发机后，要覆盖已安装的开发版并保留数据，须继续使用原调试签名。
将 keystore 保存在私有目录，并在项目忽略的 `.env` 中设置
`ZORK_ANDROID_DEBUG_KEYSTORE=/absolute/path/to/debug.keystore`；构建脚本将其
传给 Gradle 的 debug 签名配置，也适用于 `--profile`。不配置时沿用 Android
默认调试签名。不要覆盖其他项目使用的全局 keystore，也不要把密钥提交到 Git。
安装前用 SDK 的 `apksigner verify --print-certs` 核对 APK 证书。

Android 平台 TLS 验证器的 Java 类从 Cargo 锁定的 AAR 提取；不是下载另一份
独立版本。NDK compiler-rt 显式参与链接，并用 `--no-undefined` 检查缺失符号。
ELF LOAD 段使用 16 KiB 对齐，APK 保持未压缩 native 库的 16 KiB ZIP 对齐。
当前运行测试的模拟器为 4 KiB 页；16 KiB 的实际设备运行仍需另外验证。

## 验证

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_BUILD_JOBS=4 \
  cargo test --locked -p zork-android -p zork-client-core -p zork-mesh \
  --no-default-features --lib

python3 scripts/android/test_mesh.py --serial emulator-5554
```

后者安装 debug APK 和测试 APK，用应用私有的 `integration-client` 目录创建
测试身份，启动临时 Station 和 fake-model Agent。它验证真实 JNI/QUIC 请求、
订阅、去重、离线缓存、跨 Android 进程恢复、慢连接期间即时保存草稿，以及
撤销客户端授权，以及只消费 Rust 状态时的单次投递与手动重发、消息投影和缓存。测试不会访问实际节点、模型凭据或用户工作区；每次 bootstrap
仅清理自己的测试目录。手机常规 `client` 数据目录保持独立。

运行该脚本前需已有匹配当前源码的 `target/debug/zork`、`zork-station` 和
`zork-agent`。`--keep-node` 供开发者继续做真实 UI 验证，会在结果文件记录临时
节点路径和 supervisor PID，验证结束后应停止该临时节点。

输出日志和验证记录在 `artifacts/android`。界面截图使用隔离测试对话，部分
可读文案由测试夹具通过真实 Station 显式投递，不是实际用户任务结果。

液态控件通过 debug 的 `LiquidGalleryActivity` 检查完整控件与原生输入。
定向 instrumentation 使用 `LiquidControlsTest`，覆盖原生输入、来源绘制交接、退场时禁用输入、快速反向与停帧；输入区回归使用
`ComposerPresentationTest`；`LiquidPerformanceTest` 检查控件组持续切换时的
JNI/路径解码、绘制阶段与静止停帧，`ComposerPerformanceTest` 记录实际显示的
FrameMetrics，`require120=true` 要求真实 120 Hz 呈现及对应 CPU 预算。
性能测量用上述 profile 构建和编译流程，固定 APK 摘要后安装，预热后单独采样。
共享 Rust 的 `frame_budget` example 只测物理、轮廓、描边和编码，
不能代替 JNI、Canvas/GPU 或手机功耗；持续活动与静止/后台应分别测量，
热状态、刷新率和设备耗电比较保留在对应运行产物中。

## 代码边界

- `crates/zork-client-core`：桌面与 Android 共用 Station HTTP/Mesh API、DTO、
  SSE 解析、订阅重连与断线补齐、持久投递调度器、消息身份去重、分页投影、
  活动状态归并、任务修订合并和发送权限判断。桌面 `api`、`transcript`、
  `desktop::store` 重新导出同一实现，数据库格式保持兼容。
  两端都通过 core 的 `transport::start` 启动 client-only Mesh。
- core 的序列化适配器通过独立 JNI 订阅传递快照与增量；Kotlin 消费并确认已应用批次，
  不自行重连、刷新历史、去重或发送队列。桌面仍负责其专有面板的数据装配、
  滚动位置、选择和渲染；这些 UI 行为没有移入 Rust core。
- `crates/zork-android`：JNI、TLS 初始化和应用进程持有的 Rust runtime；网络
  命令与本地操作不共用等待锁。
- `crates/zork-liquid`：GPUI 与 Android 共享的物理、几何、描边、视觉状态和配方。
  可见宿主批量传入变化目标，读取同进程数值缓冲区；平台负责显示时钟、
  生命周期、路径绘制、原生输入和焦点。液态帧不走业务 JSON 通道，
  静止与后台停止调度，恢复不补算后台时间。
- `crates/zork-mesh`：`start_client` 关闭 socket 执行池和本地工作区后台循环；
  默认 `server` feature 保留 Station 行为，Android 不启用桥接编译器依赖。
- `apps/android/app`：界面、生命周期、输入、导航和安全的显式链接打开。

设备身份和会话数据放在 Android `noBackupFilesDir`，不随普通备份复制到其他
设备。此版采用应用沙箱存储；卸载后需要重新授权新身份。APK 是开发签名的
内部测试包，发布签名与应用商店分发需另行配置。

设置与视觉更新记录见 [design-qa.md](../zork-design-pc/archive/mobile-prototype/design-qa.md)，新增的真实 JNI 设置回归为 `MeshIntegrationTest.settingsManageDevice`，多宽度原生截图为 `SettingsRefreshTest`。物理手机运行 `test_mesh.py` 时使用 `--host-ip` 指定开发主机现场读取的局域网地址。

共享服务链接可从消息中打开应用内 WebView，关闭页面或离开前台即释放本地入口。协议、Agent 用法与边界见 [Mesh 服务共享](../../docs/design/external-capabilities.md#services)。
