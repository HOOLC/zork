//! Read-only next actions; platforms do not infer readiness from connection rows.
use super::*;

fn action(kind: &str, label: &str) -> Value {
    json!({"kind":kind,"label":label})
}

fn page(state: &State) -> Value {
    let (step, status, title, message, primary) = if !state.facts.supported {
        (
            "checking",
            "正在检测",
            "读取手机调试状态",
            "正在读取 Android 系统设置。",
            Value::Null,
        )
    } else if !state.settings.enabled {
        (
            "disabled",
            "尚未开启",
            "让 Station 连接这部手机",
            "完成一次 USB 激活后，每台 Station 都可以通过 Zork 跨网络安装应用、查看日志。",
            action("enable", "开始设置"),
        )
    } else if !state.active {
        (
            "checking",
            "正在恢复",
            "恢复调试连接",
            "Zork 正在恢复后台连接。",
            Value::Null,
        )
    } else {
        match state.local {
            LocalState::Checking => (
                "checking",
                "正在检测",
                "检查这部手机的状态",
                "已完成的设置会自动跳过。",
                Value::Null,
            ),
            LocalState::DeveloperDisabled => (
                "developer",
                "需要开启系统设置",
                "先开启开发者选项",
                "在系统的「关于手机」中连续点按版本号，直到开启开发者选项，再返回 Zork。",
                action("open_device_info", "打开系统设置"),
            ),
            LocalState::UsbDisabled => (
                "usb",
                "需要开启系统设置",
                "开启 USB 调试",
                "在开发者选项中开启「USB 调试」，然后返回 Zork。",
                action("open_developer_options", "打开开发者选项"),
            ),
            LocalState::ActivationRequired => (
                "activation",
                "等待 USB 激活",
                "用 USB 连接一台电脑",
                "首次使用或手机重启后，需要通过 USB 激活一次。激活完成后就可以拔线。",
                action("activation_help", "查看激活方法"),
            ),
            LocalState::WirelessOnly => (
                "activation",
                "等待 USB 激活",
                "用 USB 激活跨网调试",
                "系统无线调试已开启。跨网调试仍需通过 USB 激活一次，完成后就可以拔线。",
                action("activation_help", "查看激活方法"),
            ),
            LocalState::Ready => (
                "ready",
                "手机已就绪",
                "远程调试已开启",
                "每台 Station 都可以连接这部手机。",
                Value::Null,
            ),
        }
    };
    let note = match step {
        "activation" => Some("激活后会自动继续"),
        "ready" => Some("可拔掉 USB 线，连接会在后台保持。"),
        _ => None,
    };
    let activation_help = (step == "activation").then(|| json!({
        "kind":"activation", "title":"通过 USB 激活",
        "paragraphs":["把手机连到一台装有 ADB 的电脑，并在手机上允许 USB 调试。",
            "在电脑上用 adb devices -l 找到这部手机的 USB 序列号，替换下方的 <USB_SERIAL> 后执行："],
        "command":format!("adb -s <USB_SERIAL> tcpip {}",state.settings.port),
        "note":"执行后会自动检测，无需手动确认。"
    }));
    json!({"step":step,"status":status,"title":title,"message":message,
        "tone":if step=="ready" {"positive"} else {"neutral"},
        "primary_action":primary,
        "secondary_action":state.settings.enabled.then(||action("disable",if step=="ready" {"关闭调试"} else {"取消设置"})),
        "note":note,"show_connections":step=="ready","show_details":step=="ready",
        "empty_message":"等待 Station 连接","activation_help":activation_help})
}

fn station(lease: &Lease) -> Value {
    let (title, message) = match lease.state {
        HostState::Unauthorized => (
            "在手机上允许调试",
            format!(
                "{} 正在等待 Android 的调试授权。请解锁手机，在系统弹窗中点「允许」。",
                lease.station.name
            ),
        ),
        HostState::AdbMissing => (
            "在 Station 上安装 ADB",
            format!(
                "在 {} 上安装 Android SDK Platform-Tools，并让 Zork 能找到其中的 adb。安装后会自动继续连接。",
                lease.station.name
            ),
        ),
        HostState::BridgeUnavailable => (
            "更新 Station 上的 Zork",
            format!(
                "在 {} 上安装最新版本的 Zork，并保持它运行。更新后会自动继续连接。",
                lease.station.name
            ),
        ),
        HostState::AccessDenied => (
            "检查 Station 的设备授权",
            format!(
                "{} 不再允许这部手机访问。请在该 Station 的设备设置中检查现有授权。",
                lease.station.name
            ),
        ),
        _ => ("", String::new()),
    };
    let help =
        (!title.is_empty()).then(|| json!({"kind":"station","title":title,"paragraphs":[message]}));
    json!({"peer":lease.station.id,"origin":lease.station.origin,"name":lease.station.name,
        "state":lease.state,"state_label":lease.state.label(),"serial":lease.serial,
        "tone":if lease.state==HostState::Ready {"positive"} else if help.is_some() {"warning"} else {"neutral"},
        "help":help,"help_label":if lease.state==HostState::Unauthorized {"查看提示"} else {"查看说明"}})
}

pub(super) fn snapshot(state: &State) -> Value {
    let page = page(state);
    let mut stations: Vec<_> = state.stations.values().map(station).collect();
    stations.sort_by(|a, b| {
        a["name"]
            .as_str()
            .cmp(&b["name"].as_str())
            .then_with(|| a["peer"].as_str().cmp(&b["peer"].as_str()))
    });
    let ready = stations
        .iter()
        .filter(|station| station["state"] == "ready")
        .count();
    let notification_title = if !state.settings.enabled {
        "Mesh 调试已关闭".into()
    } else if page["step"] == "ready" && ready > 0 {
        format!("已连接 {ready} 台 Station")
    } else if page["step"] == "ready" {
        "Mesh 调试已开启".into()
    } else if page["step"] == "checking" {
        "正在检测手机调试状态".into()
    } else {
        "Mesh 调试需要操作".into()
    };
    let message = if page["step"] == "ready" {
        match stations.iter().find(|station| !station["help"].is_null()) {
            Some(station) => format!(
                "{}：{}",
                station["name"].as_str().unwrap_or("Station"),
                station["state_label"].as_str().unwrap_or_default()
            ),
            None if ready > 0 => "可以安装应用、查看日志和操作设备。".into(),
            None => "手机已就绪，等待 Station 连接。".into(),
        }
    } else {
        page["message"].as_str().unwrap_or_default().to_owned()
    };
    json!({"revision":state.revision,"settings":state.settings,"facts":state.facts,
        "local_state":state.local,"page":page,"stations":stations,"message":message,
        "notification_title":notification_title,"service_requested":state.settings.enabled && state.facts.supported})
}
