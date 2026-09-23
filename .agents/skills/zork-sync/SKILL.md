---
name: zork-sync
description: 修改或审查 zork 的客户端同步、持久化投影、游标、变更通知和重连恢复；排查同步引起的空闲高 CPU、重复请求或数据不收敛。
---

# 同步开发

先读 [同步空闲合同](../../../docs/design/synchronization.md#invalidation) 与涉及的协议/存储源码。Station 业务副本、Chat 原始消息流、附件内容缓存 各有权威和版本；消息归并用 [zork-chat-messages](../zork-chat-messages/SKILL.md)，附件用 [zork-shared-files](../zork-shared-files/SKILL.md)。

## 不变条件

- HTTP 方法不代表业务变化；POST 拉取/回执查询也不能广播失效。检查“通知 → 刷新 → 请求 → 存储 → 通知”闭环，内部导出缓存和回执记账不能触发自激循环。
- 业务投影与持久版本同事务提交；相同内容不推进版本，回滚不通知。SQLite hook 只负责唤醒。读取不承担业务协调；配置/Profile 外部来源由独立幂等流程投影，失败保留旧值并标记不可用，初始化和失败等待有界。
- 仅在 owner/epoch/scope 一致时比较 sequence；UI revision、对象 revision、同步游标不混用。先订阅再追赶，同设备/范围合并需求；已覆盖提示不排队重复请求，在途更高版本继续追赶。重连同时恢复在线状态和校验副本。
- 冻结分页保持同一批次/视图，完整落盘后推进游标。断线、过期、epoch 变化和撤权沿重置/隔离合同处理，不清库或改命令 ID 掩盖问题。需要幂等恢复的业务命令先查原回执，不自动重复提交或跨 owner 重发；普通消息发送遵循 Chat 约定。
- 客户端业务与同步生命周期经 [zork-client-boundary](../zork-client-boundary/SKILL.md)，UI 不另建列表、请求或重试。附件内容传输不套用 Station replica 游标，折叠卡片不充当原消息锚点。

## 定向验证

按 [zork-validation](../zork-validation/SKILL.md) 重建受影响二进制，选择 Station `db::sync`、core 副本测试与 `scripts/test-sync-idle.py`；隔离产物用 `ZORK_TEST_BIN_DIR`。

覆盖空读/重复协调安静、真实修改收敛后停止、突发提示合并和断线恢复；涉及分页、回执、权限时补对应边界。高 CPU 诊断同时记录调用栈、请求/通知频率和版本推进，区分业务写入、内部记账与重连；退避不代替切断错误变化来源。fixture 不证明实际设备已更新。
