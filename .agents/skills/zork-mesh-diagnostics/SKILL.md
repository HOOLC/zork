---
name: zork-mesh-diagnostics
description: 排查 zork 的设备接入、Mesh 连接、远程任务投递和后台节点生命周期问题，或验证这些模块的改动。
---

# Mesh 诊断

先读 [设备身份与生命周期](../../../docs/design/devices.md)。附件问题用 [zork-shared-files](../zork-shared-files/SKILL.md)；客户端修复遵循 [zork-client-boundary](../zork-client-boundary/SKILL.md)。

- 确定访问客户端、执行节点、数据目录和生命周期所有者。端口、服务 URL、relay/discovery 从实际配置读取，多实例使用明确 `--data`。先用 `zork mesh status`、`zork service status` 和日志定位连接、邀请/成员、执行或服务层，不因诊断重新 join、安装或停止节点。
- 区分独立 Station、客户端拥有节点和系统服务接管。supervisor 管理 Station，Agent 内嵌；`service uninstall` 撤销看护，不等于停止进程，`zork stop` 才停止节点。客户端切换不迁移执行设备或工作目录。
- 当前身份持久化与生命周期锁由 iroh 运行时拥有，不读取或迁移 Synch 旧库。分别验证正常退出、重启、附件读取和连接撤权，不以节点 ready 代替业务恢复。
- ready 后接口仍停顿时采样业务与 Mesh 工作线程，检查后台发现、TLS 平台校验等同步调用是否占满执行器；不靠增加线程数、关闭探测或放宽证书校验掩盖阻塞。
- 邀请由 Station 管理，重复 join 保留身份、Profile、任务和目录；邀请/管理凭据不进报告。成员管理节点离线不等于已有节点不能执行。relay/discovery 与账号控制面不同，客户端配置覆盖不自动改写运行中的 Station。账号目录只授予经过账号身份与设备签名验证的自动连接；安装成员与账号连接分别管理。原生 iroh 对端身份和本地成员关系是业务访问依据。公网 relay、邀请解析和重试独立于可选账号，账号退出或封禁只撤销账号自动连接，不能撤销独立安装成员。按 [中继控制面指南](../../../deploy/cloudflare/README.md) 分别验证账号会话撤销与匿名原生中继业务，不能用 WebSocket 升级证明业务可用或成员授权。切换部署核对实际中继进程和连接，Worker 发布成功不代表容器已替换。
- 业务操作丢回执时核对原请求与持久回执，未知结果不换 ID 重放；普通消息遵循 [Chat 发送约定](../../../docs/design/chat.md#身份与消息)。停止需确认真实结束。有效 Mesh 成员互信，不以旧 client/collaborate 标志追加授权；成员身份、附件来源与完整内容分别核对；已缓存不代表仍有访问权限。
- 手机 ADB 按 [调试合同](../../../docs/design/devices.md#android-debugging) 区分系统开关、本机 adbd、Mesh 桥接与各台 Station 的 ADB 授权。多台 Station 独立连接，验证单台重试或撤权不影响其他连接；Station 自动将代理连接到本机 adb server，Agent 直接使用 adb devices -l；serial 只属于该 Station 和当前代次。蜂窝模式的 USB 激活不能被无线调试开关替代。Agent 使用已授权的设备命令。

## 验证入口

修改后按 [zork-validation](../zork-validation/SKILL.md) 先构建，再选相关脚本并读取其 fixture/操作范围：

- 接入、成员、撤权：`scripts/test-mesh-enrollment.py`。
- 安装/复用：`scripts/test-native-installer.py`、`scripts/test-mesh-installer.py`；后者使用真实用户服务管理器。
- 手机账号发现与撤权：账号目录测试与 core 目录投影测试；真实 Google 登录、跨设备连接与退出撤权另做实机联调。Chat、远程工具、文件与恢复按 `scripts/test-chat-channels.py`、`scripts/test-mesh-enrollment.py` 和 `scripts/test-shared-services.py` 的实际覆盖选择。
- 网络与资源：`relay-proxy-lab` 检查代理 relay、独立 QAD 与凭据热更新，`transport-lifetime` 检查真实运行时的 FD 回收。LAN 另跑可接收 multicast 的 `live_mdns_discovers_peer_without_any_address_hint` 和运行期故障监督测试；同机地址缓存不能代替 mDNS，多网卡须检查后到的可达地址。关闭后的异步回收需有界等待，不能只测安装失败或抬高 FD 上限。
- relay 流量：`scripts/test-local-relay.py`；原生接入/后台接管：`scripts/test-mesh-experience-ui.py`。

报告故障层和证据。允许直连的测试不证明强制 relay，同机 fixture 不证明物理跨网，源码通过不证明公开 Release 资产存在。
