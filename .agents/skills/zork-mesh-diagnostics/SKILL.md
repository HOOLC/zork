---
name: zork-mesh-diagnostics
description: 排查 zork 的设备接入、Mesh 连接、远程任务投递和后台节点生命周期问题，或验证这些模块的改动。
---

# Mesh 诊断

先读 [设备身份与生命周期](../../../docs/design/devices.md)。文件/远端 Skill 问题用 [zork-shared-files](../zork-shared-files/SKILL.md)；客户端修复遵循 [zork-client-boundary](../zork-client-boundary/SKILL.md)。

- 确定访问客户端、执行节点、数据目录和生命周期所有者。端口、服务 URL、relay/discovery 从实际配置读取，多实例使用明确 `--data`。先用 `zork mesh status`、`zork service status` 和日志定位连接、邀请/成员、执行或服务层，不因诊断重新 join、安装或停止节点。
- 区分独立 Station、客户端拥有节点和系统服务接管。supervisor 管理 Station，Agent 内嵌；`service uninstall` 撤销看护，不等于停止进程，`zork stop` 才停止节点。客户端切换不迁移执行设备或工作目录。
- 启动等待分开核对本地运行时、历史恢复、本地发布与对端推送。正常关闭后重启、异常退出和数据库回滚分别验证；优化正常恢复不能跳过旧数据库的自身签名历史校验，也不能只测桥接发布而漏掉后续源扫描入队。
- ready 后接口仍停顿时采样业务与 Mesh 工作线程，检查后台发现、TLS 平台校验等同步调用是否占满执行器；不靠增加线程数、关闭探测或放宽证书校验掩盖阻塞。
- 邀请由 Station 管理，重复 join 保留身份、Profile、任务和目录；邀请/管理凭据不进报告。成员管理节点离线不等于已有节点不能执行。relay/discovery 与账号控制面不同，客户端配置覆盖不自动改写运行中的 Station。账号问题按 [中继控制面指南](../../../deploy/cloudflare/README.md) 核对账号所属 Profile、签发 origin、服务端撤销及已有连接；桌面客户端与其内置 Station 共用账号，独立 Station 分开登录。验收从真实客户端登录入口开始，并覆盖未连接 Mesh 时登录、待处理邀请接续和退出后的运行中 Station；不能用本地文件删除或一次 WebSocket 升级证明完整退出与续期。切换部署还须核对实际中继进程和旧连接，Worker 发布成功不代表容器已替换。
- 业务操作丢回执时核对原请求与持久回执，未知结果不换 ID 重放；普通消息遵循 [Chat 发送约定](../../../docs/design/chat.md#身份与消息)。停止需确认真实结束。有效 Mesh 成员互信，不以旧 client/collaborate 标志追加授权；成员身份、来源元数据/CAS 与 Agent Skill 来源分别核对，树中可见不表示已下载或已绑定。
- 手机 ADB 按 [调试合同](../../../docs/design/devices.md#android-debugging) 区分系统开关、本机 adbd、Mesh 桥接与各台 Station 的 ADB 授权。多台 Station 独立连接，验证单台重试或撤权不影响其他连接；Station 自动将代理连接到本机 adb server，Agent 直接使用 adb devices -l；serial 只属于该 Station 和当前代次。蜂窝模式的 USB 激活不能被无线调试开关替代。Agent 使用内置 [Android 调试 Skill](../../../crates/agent/skills/android-debugging/SKILL.md)。

## 验证入口

修改后按 [zork-validation](../zork-validation/SKILL.md) 先构建，再选相关脚本并读取其 fixture/操作范围：

- 接入、成员、撤权：`scripts/test-mesh-enrollment.py`。
- 安装/复用：`scripts/test-native-installer.py`、`scripts/test-mesh-installer.py`；后者使用真实用户服务管理器。
- 客户端成员状态/outbox：`scripts/test-client-mesh.py`；远端恢复/回执：`scripts/test-remote-workers.py`。
- relay 流量：`scripts/test-local-relay.py`；原生接入/后台接管：`scripts/test-mesh-experience-ui.py`。

报告故障层和证据。允许直连的测试不证明强制 relay，同机 fixture 不证明物理跨网，源码通过不证明公开 Release 资产存在。
