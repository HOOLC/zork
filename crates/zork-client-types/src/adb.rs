//! Phone observations and the authenticated ADB bridge protocol.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Facts {
    #[serde(default)]
    pub supported: bool,
    #[serde(default)]
    pub application_id: String,
    pub developer_enabled: Option<bool>,
    pub usb_enabled: Option<bool>,
    pub wireless_enabled: Option<bool>,
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalState {
    #[default]
    Checking,
    DeveloperDisabled,
    UsbDisabled,
    ActivationRequired,
    WirelessOnly,
    Ready,
}
impl LocalState {
    pub fn message(self) -> &'static str {
        match self {
            Self::Checking => "正在检测本机 ADB…",
            Self::DeveloperDisabled => "请在系统设置中启用开发者选项，返回 Zork 后会自动继续。",
            Self::UsbDisabled => "请在开发者选项中开启 USB 调试。",
            Self::ActivationRequired => {
                "请用 USB 连接一台电脑，允许 USB 调试后，由电脑执行 ADB 网络激活。激活后可以拔线，手机使用蜂窝网络连接 Mesh；手机重启后需重新激活。"
            }
            Self::WirelessOnly => {
                "检测到无线调试。蜂窝网络调试需要先通过 USB 激活 ADB 网络端口，系统的无线调试开关不能替代这一步。"
            }
            Self::Ready => "手机 ADB 已就绪，每台 Station 都可以连接。",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    #[default]
    Connecting,
    BridgeUnavailable,
    AccessDenied,
    AdbMissing,
    Unauthorized,
    Offline,
    Ready,
}
impl HostState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "正在连接",
            Self::BridgeUnavailable => "需要更新 Zork",
            Self::AccessDenied => "设备授权不可用",
            Self::AdbMissing => "尚未安装 ADB",
            Self::Unauthorized => "等待手机授权",
            Self::Offline => "等待重新连接",
            Self::Ready => "已连接",
        }
    }
    pub fn message(self) -> &'static str {
        match self {
            Self::Connecting => "正在通过 Mesh 连接开发电脑…",
            Self::BridgeUnavailable => {
                "开发电脑尚未提供兼容的 Mesh ADB 接口，请更新并重启该电脑上的 Zork。"
            }
            Self::AccessDenied => "该 Station 已拒绝此手机的访问，请检查它的设备授权。",
            Self::AdbMissing => {
                "开发电脑未找到 ADB，请安装 Android SDK Platform-Tools，并让 Zork 能通过 PATH 找到 adb。"
            }
            Self::Unauthorized => "请解锁手机，在系统弹窗中允许这台开发电脑进行 USB 调试。",
            Self::Offline => {
                "开发电脑尚未连上 ADB，正在自动重试；请保持手机解锁并检查系统授权弹窗。"
            }
            Self::Ready => "开发电脑已连接 ADB，可以安装应用、启动调试和读取日志。",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Advertisement {
    pub generation: String,
    pub name: String,
    pub activation_port: u16,
    pub bridge_application_id: String,
    pub local: LocalState,
}
