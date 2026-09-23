---
name: zork-native-release
description: 制作或验证 zork 原生节点发布包、安装器及升级流程；区分四平台节点发布与桌面应用分发。
---

# 原生节点发布

先读 [原生发布合同](../../../docs/guides/native-releases.md)、`scripts/build/native-release.py`、`.github/workflows/release.yml` 和 `package.json`；版本、组件、平台和流水线以当前文件为准。

- 节点包含 supervisor、Station、独立 Agent 工具和 GitHub helper，直接使用 iroh 传输，不依赖 Node/npm 或额外传输 daemon。生产 supervisor 仅管理内嵌 Agent 的 Station，独立工具 binary 不代表第二个生产 Agent 进程。
- 使用冻结依赖、`cargo build --locked --release -p zork -p zork-station -p zork-agent-server -p zork-gh`，再执行 `python3 scripts/build/native-release.py stage`；stage 不用 debug 产物兜底。构建环境按 [zork-validation](../zork-validation/SKILL.md) 加载。
- 本机只验证当前平台；assemble 核对四平台完整组件、版本、manifest 与摘要。安装/复用和升级分别使用 workflow 中对应的 native/mesh installer、native/station upgrade 测试。
- PR/分支产物是 workflow artifacts，匹配 package 版本的 `v*` tag 才启用发布。准备包不包含推 tag/公开发布授权，不覆盖已发布版本；按已有授权继续流程。
- 安装页面只为已发布且完整的四平台版本生成下载命令；邀请凭据留在 URL 片段，不发送给页面服务器或下载源。原生端使用当前身份、附件存储和协议，不提供 Synch 数据迁移或旧协议适配。
- 安装/join、`zork update` 重启已暂存二进制、`zork upgrade` 更换完整版本是不同操作。升级失败查目标数据目录的 `logs/update.log`、`run/update.json` 与旧二进制；恢复 app/binary 不自动回滚新版本写入的数据。
- test 安装包使用独立配置的局域网对象存储，与构建缓存分开；用 `scripts/build/publish-test.py` 固定候选并给 test 客户端打包分发元数据。缺配置不得回退公开发布，部分平台候选不得冒充四平台发布。
- 桌面 `.app` 另走 `scripts/package-macos-client.py`，本地签名不代表公开签名/公证完成。固定当前 app、显式导出和临时产物清理遵循 validation；个人测试设备安装按本地环境规则。
- 本机 release/dev 的观察、提升和数据恢复使用[自救流程](../../../docs/guides/release-dev-recovery.md)。提升复用已观察的完整产物，不用一次新的优化构建冒充原候选；公开版本号不能证明正在运行的 commit 或 helpers 与构建一致。

报告本机构建、四平台流水线、组装和公开下载各自状态。涉及 PR 时等最新 head 的 CI、review blocker 和 mergeability 达到可合并；可合并不自动授权合并。
