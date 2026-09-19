---
name: zork-client-subscriptions
description: 设计、实现或审查 zork Rust core 到 GPUI、Android/JNI、Web 的状态订阅、增量传递和高频更新调度；守护版本基线、慢消费者、取消与性能边界。纯视觉动效或仅服务端网络通知不使用本 skill。
---

# 客户端状态订阅

先读 [订阅合同](../../../docs/design/state-subscriptions.md)，业务职责遵循 [core/UI 边界](../zork-client-boundary/SKILL.md)。追踪提交 → 路由 → 调度/编码 → 应用/确认 → 呈现；平台接入和 wire 字段查源码。

## 一致性

- 先注册再读，空状态也有 Reset；状态、版本和变化边界一起提交。业务原子性的跨 source 结果由 core 聚合。
- 增量相对消费者**已应用**版本。每个订阅最多一个未应用批次，按顺序完整应用后确认；读取/编码不推进基线，取消或失败保留旧基线，不匹配就 Reset 当前范围。
- UI revision、对象 revision、同步游标独立。source/授权/投影更换隔离旧批次；FFI 句柄与 generation 覆盖关闭及 A→B→A。释放观察不取消业务或销毁其他消费者的控制器。
- 唤醒是提示；确认后重查新版本再挂起。等待可取消且空闲不轮询，持锁时不调用平台或编码。日志按条数与字节有界，慢消费者不阻塞快消费者。
- reducer 判断 no-op，不能靠发布层深比较整棵状态。只合并提示或自足最新值，依赖前序的 raw patch 不能随意 conflation；业务完成不依赖 UI 回调。

## 高频与平台

- 写侧记录变化、按 topic/key 路由，按记录共享并维护增量索引；流式正文按块处理。避免全历史扫描、前插重编号和整列表复制；`Arc<Vec<Arc<T>>>` 仍可能复制 N 个指针。
- 合并发生在 diff/编码/呈现转换之前。GPUI 用 `FrameDelivery`；`on_next_frame` 已唤醒帧源，不额外调用依赖当前绘制视图的 `request_animation_frame`。Android 等待不持命令锁，编解码不占主线程帧回调。
- 执行历史由节点持有，core 按需在 RAM 归并；累计统计来自 snapshot 与后续提交，不拉全档案或建客户端历史库；History 阅读页的已加载调用统计须明确范围，不冒充累计概览。首连/重连先 snapshot，详情分页不改变概览；普通 Chat 不开启独立 History loader。
- 大列表窗口与所选详情有界，目录快照不冒充逐行 delta。OS 通知不等显示帧，发送前 core 重验隐私/权限，接受不标记已读；资源和文件复用原控制器及失效规则。

## 验证

覆盖首次空值、重复输入、突发/慢消费、discard/ack 与写入交错、切换和撤权；最终平台镜像等于 core 投影。性能按影响选择 1,000/10,000/100,000 条混合数据及追加、远距离更新、前插、仅状态变化，分别测耗时、字节/分配、唤醒、峰值和重置。

按 [验证规则](../zork-validation/SKILL.md) 独立于构建测量并报告源码/产物；微基准不等于设备 FPS。同步用 [zork-sync](../zork-sync/SKILL.md)，可见呈现用 [UI parity](../zork-ui-parity/SKILL.md)。
