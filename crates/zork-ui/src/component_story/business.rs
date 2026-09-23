//! Labels for native component stories.

pub fn is_business(family: &str) -> bool {
    matches!(
        family,
        "device-name"
            | "welcome"
            | "node-directory"
            | "connection"
            | "model"
            | "client"
            | "device"
            | "mesh"
            | "enrollment"
            | "message-interaction"
            | "conversation"
            | "history-details"
            | "history"
            | "comments"
            | "attachment"
            | "tooltip"
            | "markdown"
            | "activity"
            | "composer"
            | "resources"
            | "notifications"
            | "appearance"
            | "data-settings"
            | "browser"
            | "attachment-viewer"
            | "chat-navigation"
            | "message-reader"
            | "conversation-files"
    )
}

pub fn state_label(state: &str) -> String {
    let state = state.trim_end_matches("-compact");
    match state {
        "primary" => "主要操作",
        "secondary" => "次要操作",
        "with-icon" => "带图标",
        "focus" => "键盘焦点",
        "value" => "已输入",
        "secret" => "密码",
        "off" => "关闭",
        "on" => "开启",
        "disabled-off" => "禁用 · 关闭",
        "disabled-on" => "禁用 · 开启",
        "selected" => "选中",
        "selected-focus" => "选中 · 键盘焦点",
        "selected-hover" => "选中 · 悬停",
        "gap" => "分组间距",
        "closed" => "收起",
        "open" => "展开",
        "default" => "默认",
        "standard" => "标准",
        "scroll" => "内容滚动",
        "success" => "成功",
        "warning" => "警告",
        "notice" => "提示",
        "overview" => "交互总览",
        "form" => "表单",
        "inline" => "行内",
        "button" => "按钮",
        "gallery" => "交互展台",
        "all" => "全部",
        "wordmark" => "字标",
        "icon" => "图标",
        "morph" => "形变",
        "header" => "页头",
        "linked" => "联动",
        "first-chat" => "首次使用",
        "choose-chat" => "选择对话",
        "background" => "后台运行",
        "pairing" => "配对表单",
        "toolbar" => "成员与文件菜单",
        "tabs" => "多个页面",
        "agent" => "成员详情",
        "entry" => "执行记录",
        "idle" => "空闲",
        "active" => "工作中",
        "automatic" => "自动",
        "minimum" => "最小高度",
        "maximum" => "最大高度",
        "custom" => "自定义高度",
        "applications" => "应用列表",
        "interactive" => "交互示例",
        "document" => "文档",
        "literal" => "原文",
        "markdown" => "格式化正文",
        "files" => "文件列表",
        "pages" => "页面列表",
        "menu" => "菜单",
        "grid" => "网格",
        "preview" => "预览",
        "unread" => "未读",
        "offline" => "离线",
        "creator" => "创建者",
        "enabled" => "已启用",
        "disabled" => "已关闭",
        "muted" => "已静音",
        "denied" => "未获授权",
        "busy" => "处理中",
        "services" => "服务",
        "list" => "列表",
        "create" => "新建",
        "edit" | "editing" => "编辑",
        "detail" => "详情",
        "provider" => "供应商",
        "protocol" => "接口类型",
        "dropdown" => "选择模型",
        "signed-out" => "未登录",
        "signed-in" => "已登录",
        "not-started" => "未启动",
        "preparing" => "准备中",
        "direct" => "直连",
        "relay" => "中继",
        "connecting" => "连接中",
        "stopping" => "停止中",
        "revoked" => "访问已撤销",
        "loading" => "加载中",
        "error" => "错误",
        "running" => "运行中",
        "stopped" => "已停止",
        "connected" => "已连接",
        "empty" => "空状态",
        "manual" => "手动添加",
        "start" => "开始",
        "command" => "连接命令",
        "expired" => "已过期",
        "collapsed" => "收拢",
        "expanded" => "展开",
        "compose" => "添加评论",
        "queued" => "待发队列",
        "file" => "文件",
        "image" => "图片",
        "unavailable" => "不可用",
        "row" => "列表行",
        "message-image" => "消息图片",
        "message-document" => "消息文档",
        "leader" => "领队",
        "task" => "任务",
        "hover" => "悬停",
        "paragraph" => "段落",
        "heading" => "标题",
        "quote" => "引用",
        "code" => "代码",
        "table" => "表格",
        "failed" => "失败",
        "done" => "完成",
        "approval" => "等待批准",
        "approved" => "已批准",
        "declined" => "已拒绝",
        "cancelled" => "已取消",
        "input" => "填写",
        "prefilled" => "已填写",
        "update" => "修改",
        "login" => "登录",
        "login-device" => "设备登录",
        "login-callback" => "登录回调",
        "login-completed" => "登录完成",
        "completed" => "已完成",
        "long" => "长内容",
        "messages" => "消息",
        "composer" => "消息输入",
        "history" => "执行历史",
        _ => state,
    }
    .to_owned()
}
