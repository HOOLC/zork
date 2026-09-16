# 独立手机推送方案

状态：待决定、未实施。与 [当前系统通知](../design/notifications.md) 和 Android 的前台服务连接分开；本文不定义已经具备的投递能力。

目标包含国内无 GMS 设备。候选包括具备 GMS/网络条件的 FCM、获准接入的厂商系统通道，以及需用户配置 distributor 的 UnifiedPush。分发方式、厂商资格、AI/任务通知分类和真实网络到达率尚待确认，不沿用旧研究的报价或准入结论。

共同链路为：权威节点提交事件与 outbox → 授权通知网关 → 一个活动通道 → 系统显示 → 点击后 Mesh 补齐。节点仅需出站连接；网关持有通道凭据和设备地址，不向供应商发送 Chat 正文、代码或密钥。供应商仍会看到必要投递元数据。

系统展示不能依赖先唤醒完整 Mesh 并翻页同步。自动展示时应用可能不运行，因此总开关/免打扰必须同步到发送端，离线修改显示待同步；客户端过滤不能代替发送端策略。

安装实例、Mesh 身份和通道 token 分开，处理刷新、重装、解绑、撤权和通道切换。前台 Mesh 与推送共用事件 ID；网关接受、供应商接受、设备显示、点击和已读分别确认。过期与无效 token 有界清理，未知到达不无限重试。

实施前核对并小规模实测：[FCM 要求](https://firebase.google.com/docs/cloud-messaging/android/get-started)、[Android 通知渠道](https://developer.android.com/develop/ui/compose/notifications/channels)、[后台限制](https://developer.android.com/training/monitoring-device-state/doze-standby)、[UnifiedPush](https://unifiedpush.org/users/distributors/)。按真实 ROM、分发方式、锁屏、强停、重启和网络切换记录延迟与重复率；不由一条演示通知推断覆盖。
