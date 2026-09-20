---
name: zork-shared-files
description: 开发、审查或排查 zork 的 Synch 共享文件树、附件文件权威、固定版本读取保存及远端文件物化时使用；守护统一目录、授权和缓存边界。纯文件预览样式不使用。
---

# 共享文件与固定版本

先读 [共享文件与附件](../../../docs/design/shared-files.md)，Skill 消费用 [运行时 skill](../zork-agent-skills/SKILL.md)。只处理本次文件功能，不自动重排或迁移用户数据。

## 空间与权威

- 文件共享使用独立、默认空的用户目录与 space，core 直接查询这个范围；系统工作区、Session 和自动生成附件留在 Station 业务区域，不发布到 Synch 空间，Skill 用专用来源和 UI。发布根不重叠，不靠目录名筛选或枚举全部 space 后排除内部项，不映射 Station 数据根。默认隐藏来源，core 保留真实身份。
- 当前 Mesh 完全互信；文件浏览、Skill 自动发现、Station 管理权限各有职责，不新增 space 私有隔离。物理布局查当前合同/代码，不能把现状写成永久规则。
- Agent 从执行环境获取共享目录，用普通本地文件操作处理文件，Synch 自动发布目录变化。运行时 `file-sharing` Skill 说明文件归属和共享语义；复制、覆盖与落盘方式由 Agent 按任务处理，不另设共享写入工具或规定唯一操作流程。
- 消费现有 Synch 统一树，不逐 Station 拼目录。附件先 staging 校验、原子发布文件，再提交带固定引用的源消息，发布顺序遵循 [Chat 约定](../../../docs/design/chat.md#身份与消息)；允许未引用完整文件，不允许半文件消息或永久二次导出。
- 同路径/内容可有多个 origin，不同内容保留版本；默认沿 Synch Newest。预览/读取/保存固定选定文件 root，目录消费固定 origin/snapshot 并继承到资源、分页和物化。来源在线、访问端就绪和已缓存分别表达，未知不猜。

## 读取、保存与恢复

- 真实目录由扫描器发布，覆盖空目录生命周期、忽略与模式变化。持续变化不能无限延迟发布，查询不启动扫描/反熵。游标绑定范围和真实变化，不能用最大 sequence/数量遗漏迟到记录；内存有界不表示查询常数成本。目录、预览、内容缓存和完整保存各有边界。
- core 的共享控制器管理筛选、分页、读取与保存，平台只采集输入和目的地。先订阅再读，失活暂停查询；连接/页面代次隔离旧结果，同权威的系统保存选择器保留已准备票据。
- 撤权、来源退出或连接替换使对应预览、票据和未应用批次失效；已导出文件不能收回。文件浏览不重放执行历史或新建客户端历史库。
- materialize 只为执行固定子树版本到私有缓存，不安装 Skill 或重新发布。活动进程不淘汰已返回路径，链接不得逃出子树，socket 不物化执行，执行位不能凭空推测。

## 验证

遵循 [客户端边界](../zork-client-boundary/SKILL.md)、[订阅](../zork-client-subscriptions/SKILL.md) 和 [验证规则](../zork-validation/SKILL.md)。`shared_files_mesh` 用 `ZORK_TEST_STATION_BIN` 固定新产物，覆盖旧业务源退役、多来源/版本、固定保存、Skill 执行、重连和撤权；附件下载须保持固定字节，不能为读取再创建永久发布副本。

Android 文件交付用 `scripts/android/test_file_delivery.py`，先重建 Station 和 APK，指定独立模拟器；核对 JNI、目录元数据收敛、实际图片呈现和完整导出字节。历史 CAS 占用先按[回收指南](../../../docs/guides/file-cas-recovery.md)做只读审计，不能把停止发布等同于原文件或全部 CAS 可删除。

大目录用 `directory_with_100000` 独立测量，平台检查选择 `headless_shared_files` / `SharedFilesTest`。元数据、UI、同机多节点和物理跨网的证据分开。
