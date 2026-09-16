# Android 本机操作

本文用于选择 Android 本机动作，区分可直接执行、需用户授权和系统不允许的操作。按普通第三方应用、Android 10 及以上、现代 target SDK 判断，不预设 ADB、root、系统签名或设备所有者权限。下表描述 Android 平台能力；脚本实际可调用的宿主接口以 `chat.send.android_script` 的工具帮助为准，厂商兼容性需要实测。

## 卡片的执行方式

用 `chat.send.android_script` 发送标题、说明和 JavaScript 源码，兼容的 Android 客户端显示本机执行按钮。执行与消息发布的关系遵循 [本机脚本卡片约定](../design/chat.md#本机脚本卡片)：原生回调和日志只留本机，用户自行向 Agent 说明后续情况。

## 动作脚本的表达

直接使用 JavaScript 源码，按 ES module 执行。多步操作、变量、条件、循环、函数和异步等待使用 JavaScript 自身的语法。卡片承载平台、脚本接口版本与源码，标题、说明和按钮文案属于呈现。宿主接口的参数、支持范围和上限以工具帮助为准。

“打开开发者选项”的脚本：

```javascript
await android.startActivity({
  action: "android.settings.APPLICATION_DEVELOPMENT_SETTINGS",
});
```

`android` 是客户端向脚本提供的宿主接口，不是 JavaScript 自带对象。示例中的调用经 core 定义的平台能力进入 Kotlin 适配，构造对应 Intent 并启动。其他本机能力按同样方式接入，权限判断、参数校验和执行生命周期由 core 管理；Android 平台适配提供系统授权界面、原生调用及回调。

脚本由 core 中的 QuickJS 引擎执行，原生调用经 core 定义的能力接口进入 Kotlin 适配。运行具有内存、CPU 时间、总时长和原生调用次数上限。[QuickJS 嵌入接口](https://bellard.org/quickjs/quickjs.html#QuickJS-C-API)

系统授权与异步回调在本机完成，并作为 Promise 的完成或异常交回脚本。未捕获异常结束本次脚本并在本机提示；打开设置页的调用完成只表示发起跳转，不等待用户在目标页面操作。脚本权限仍受 Android 应用权限约束。未知接口版本或不支持的平台只读展示，不自动运行消息中的代码。脚本执行结果不回传 Agent，也不接续挂起的 Agent 调用。

## 能直接执行的本机动作

这些动作不需要每次弹出系统授权框；部分需要应用在 Manifest 声明普通权限。

| 操作                 | 原生入口                                             | 实际能力与条件                                                                                                                                                                                                                                                                                                 |
| -------------------- | ---------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 复制文字、清空剪贴板 | `ClipboardManager.setPrimaryClip / clearPrimaryClip` | 直接修改系统剪贴板；读取另受输入焦点限制，不能承诺后台持续读取。[剪贴板 API](https://developer.android.com/reference/android/content/ClipboardManager)                                                                                                                                                         |
| 开关手电筒           | `CameraManager.setTorchMode`                         | 有可用闪光灯时直接开关，无需打开相机；资源被占用时可能失败，状态由 torch callback 返回。[相机 API](<https://developer.android.com/reference/android/hardware/camera2/CameraManager#setTorchMode(java.lang.String,boolean)>)                                                                                    |
| 调节媒体音量         | `AudioManager.setStreamVolume`                       | 声明普通权限 `MODIFY_AUDIO_SETTINGS`。固定音量设备可能无效；涉及勿扰状态的调整需要额外访问权。[音量 API](<https://developer.android.com/reference/android/media/AudioManager#setStreamVolume(int,int,int)>)、[权限](https://developer.android.com/reference/android/Manifest.permission#MODIFY_AUDIO_SETTINGS) |
| 振动                 | `Vibrator.vibrate`                                   | 声明普通权限 `VIBRATE`，设备须有振动器。[权限](https://developer.android.com/reference/android/Manifest.permission#VIBRATE)                                                                                                                                                                                    |
| 读写应用自己的文件   | 应用文件目录、应用创建的媒体                         | 可直接读写有权访问的文件；不因此取得其他应用私有目录或整个存储空间的访问权。[存储权限范围](https://developer.android.com/privacy-and-security/minimize-permission-requests)                                                                                                                                    |

手电筒与拍照应分开判断：公开的 `setTorchMode` API 未要求 `CAMERA`，当前 AOSP 的普通相机 torch 调用路径也没有检查该运行时权限。这是文档与源码结论，厂商兼容性仍须实测。[AOSP CameraService](https://android.googlesource.com/platform/frameworks/av/+/main/services/camera/libcameraservice/CameraService.cpp)

## 能直接打开的系统页面

这些入口能打开页面或面板，页面里的开关与授权仍由用户操作。打开页面不能作为开关已开启的证据。厂商可能缺少部分入口，调用须处理没有匹配 Activity 的情况。[系统设置 Intent](https://developer.android.com/reference/android/provider/Settings)

| 页面                                                               | 原生入口                                                                                 | 能完成什么                                                                                                                   |
| ------------------------------------------------------------------ | ---------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- |
| 开发者选项                                                         | `Settings.ACTION_APPLICATION_DEVELOPMENT_SETTINGS`                                       | 打开开发者选项；不自动启用开发者模式、USB 调试或无线调试                                                                     |
| 关于手机                                                           | `Settings.ACTION_DEVICE_INFO_SETTINGS`                                                   | 打开设备信息页；不能替用户完成点击版本号的启用流程                                                                           |
| 指定应用详情                                                       | `Settings.ACTION_APPLICATION_DETAILS_SETTINGS`，`package:` URI                           | 打开应用详情，供用户管理权限、存储等                                                                                         |
| 指定应用通知设置                                                   | `Settings.ACTION_APP_NOTIFICATION_SETTINGS`，`EXTRA_APP_PACKAGE`                         | 打开应用通知设置；不自动允许通知                                                                                             |
| Wi-Fi、蓝牙、定位、显示、声音、日期时间                            | 对应 `Settings.ACTION_*_SETTINGS`                                                        | 跳转相应系统设置页                                                                                                           |
| 无障碍、修改系统设置、通知读取、使用情况访问、悬浮窗、未知来源安装 | 对应公开 Settings 授权管理入口                                                           | 打开特殊访问权的管理页；不替应用授予权限                                                                                     |
| 网络、Wi-Fi、NFC、音量快捷面板                                     | `Settings.Panel.ACTION_INTERNET_CONNECTIVITY / ACTION_WIFI / ACTION_NFC / ACTION_VOLUME` | Android 10 起可打开局部面板，让用户调整。[设置面板](https://developer.android.com/reference/android/provider/Settings.Panel) |

## 能发起的其他应用与系统交互

| 操作                             | 原生入口                                                                    | 是否还需要用户操作                                                                                                                                                                                                   |
| -------------------------------- | --------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 打开网页、地图位置、应用公开页面 | `Intent.ACTION_VIEW` 与对应 URI / deep link                                 | 可直接跳转；目标应用须提供相应入口，可能出现应用选择器。不能打开对方未公开的任意内部页面。[通用 Intent](https://developer.android.com/guide/components/intents-common)                                               |
| 打开拨号盘并填入号码             | `ACTION_DIAL`，`tel:`                                                       | 不需要 `CALL_PHONE`；拨出仍由用户完成。[拨号 Intent](https://developer.android.com/guide/components/intents-common#Phone)                                                                                            |
| 填好短信或邮件                   | `ACTION_SENDTO`，`smsto:` / `mailto:`                                       | 打开编辑页，用户点击发送；不等于消息已发送。[消息 Intent](https://developer.android.com/guide/components/intents-common)                                                                                             |
| 分享文字、图片、文件             | `ACTION_SEND / ACTION_SEND_MULTIPLE`、系统 Sharesheet                       | 用户选择接收应用及后续动作；文件通过有权访问的 URI 分享。[分享](https://developer.android.com/guide/components/intents-common)                                                                                       |
| 选择照片、视频                   | Photo Picker                                                                | 用户选择具体媒体后返回可读 URI；不必申请整个相册的读取权限。[照片选择与权限](https://developer.android.com/privacy-and-security/minimize-permission-requests)                                                        |
| 选择文件、目录，指定文件保存位置 | `ACTION_OPEN_DOCUMENT / ACTION_OPEN_DOCUMENT_TREE / ACTION_CREATE_DOCUMENT` | 用户选择并授予对应 URI 的权限，之后可在授权范围内读写；目录授权也不覆盖系统禁止选择的位置。[文件选择](https://developer.android.com/training/data-storage/shared/documents-files)                                    |
| 调起系统相机拍照、录像           | `ACTION_IMAGE_CAPTURE / ACTION_VIDEO_CAPTURE`                               | 用户在相机中完成拍摄。未声明 `CAMERA` 时可委托相机；若本应用已声明却未获授权，调用可能抛 `SecurityException`。[委托拍摄权限](https://developer.android.com/privacy-and-security/minimize-permission-requests)        |
| 填好新联系人、日历事件           | 联系人或日历的 `ACTION_INSERT`                                              | 用户确认保存；委托方式不要求本应用取得写通讯录、写日历权限。[联系人](https://developer.android.com/identity/providers/contacts-provider)、[日历](https://developer.android.com/identity/providers/calendar-provider) |
| 设置闹钟、启动倒计时             | `AlarmClock.ACTION_SET_ALARM / ACTION_SET_TIMER`                            | 声明 `SET_ALARM`；参数完整时可用 `EXTRA_SKIP_UI` 请求跳过中间界面，最终取决于时钟应用，不能仅凭 Intent 启动成功宣称已设置。[闹钟与计时器](https://developer.android.com/reference/android/provider/AlarmClock)       |

## 用户授予运行时权限后能直接执行

用户可以拒绝、撤销或只授权本次使用；相机、麦克风、定位还受前后台状态与系统隐私开关约束。这里按前台执行判断。[运行时权限](https://developer.android.com/training/permissions/requesting)、[相机、麦克风与定位访问](https://developer.android.com/training/permissions/explaining-access)

| 操作                                 | 原生入口                              | 必要权限与边界                                                                                                                                                                                    |
| ------------------------------------ | ------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 应用内拍照、录像、扫码               | CameraX / Camera2；扫码另做图像识别   | `CAMERA`；带声音录像还需 `RECORD_AUDIO`                                                                                                                                                           |
| 录音                                 | `MediaRecorder / AudioRecord`         | `RECORD_AUDIO`                                                                                                                                                                                    |
| 获取当前位置                         | `LocationManager` 等定位 API          | `ACCESS_COARSE_LOCATION` 或 `ACCESS_FINE_LOCATION`；用户可以只给近似位置，后台定位是额外条件。[定位权限](https://developer.android.com/develop/sensors-and-location/location/permissions/runtime) |
| 发布本应用通知                       | `NotificationManager.notify`          | Android 13 起普通通知需要 `POST_NOTIFICATIONS`；还受应用及频道开关影响。[通知授权](https://developer.android.com/develop/ui/compose/notifications/notification-permission)                        |
| 直接拨出电话                         | `ACTION_CALL`                         | `CALL_PHONE`；这是拨出动作，与打开拨号盘不同，仍可能受 SIM 等系统条件影响。[拨打电话](https://developer.android.com/guide/components/intents-common#Phone)                                        |
| 查询、新增、修改、删除联系人         | `ContactsContract`、`ContentResolver` | 按读写申请 `READ_CONTACTS / WRITE_CONTACTS`；通过可写的联系人数据表操作。[联系人权限](https://developer.android.com/identity/providers/contacts-provider)                                         |
| 查询、新增、修改、删除日历事件与提醒 | `CalendarContract`、`ContentResolver` | 按读写申请 `READ_CALENDAR / WRITE_CALENDAR`，目标日历须可写。[日历权限](https://developer.android.com/identity/providers/calendar-provider)                                                       |

## 需要独立特殊授权或其他条件

| 操作                                       | 原生入口                                                 | 必须具备的条件                                                                                                                                                                                                                                                                                             |
| ------------------------------------------ | -------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 修改系统亮度、自动旋转、休眠超时等         | `Settings.System` 对应可写项                             | 声明 `WRITE_SETTINGS`，用户在专门页面允许后 `Settings.System.canWrite` 为真；不允许任意修改 Secure / Global 设置。[可写设置](https://developer.android.com/reference/android/provider/Settings.System)                                                                                                     |
| 截取或录制其他应用画面                     | `MediaProjection`                                        | 每次建立采集会话都须用户同意；新系统还有前台服务类型要求。它提供画面，不提供点击、输入能力。[屏幕采集授权](https://developer.android.com/media/grow/media-projection#user-consent)                                                                                                                         |
| 读取其他应用可访问的界面结构               | `AccessibilityService` 的窗口与节点 API                  | 用户手动启用应用提供的无障碍服务，并配置读取窗口内容能力；对方暴露的节点不保证完整或及时。[无障碍服务](https://developer.android.com/reference/android/accessibilityservice/AccessibilityService)                                                                                                          |
| 点击、滑动、填写可编辑控件、返回、回到桌面 | 无障碍节点动作、`dispatchGesture`、`performGlobalAction` | 用户启用无障碍服务；手势还需声明相应服务能力。受控件支持与系统限制，不是普通应用天然具备的权限。[无障碍动作](https://developer.android.com/reference/android/accessibilityservice/AccessibilityService)                                                                                                    |
| 直接发送短信                               | `SmsManager.sendTextMessage`                             | `SEND_SMS` 是 hard restricted 权限，安装器须允许授予，再取得用户授权。还需可用短信订阅；不能只加 Manifest 就保证可用。[发送 API](https://developer.android.com/reference/android/telephony/SmsManager)、[受限权限](https://developer.android.com/reference/android/Manifest.permission#SEND_SMS)           |
| 安装 APK、更新应用                         | `PackageInstaller`                                       | 普通首次安装通常需要允许该来源安装及系统确认。无交互更新仅在安装者身份、更新所有权、权限与目标版本等条件同时满足时可行；始终要处理 `STATUS_PENDING_USER_ACTION`。[安装用户操作条件](<https://developer.android.com/reference/android/content/pm/PackageInstaller.SessionParams#setRequireUserAction(int)>) |

## 普通应用不能直接替用户完成

| 想做的动作                                       | 平台边界与可用替代入口                                                                                                                                                                                                                                          |
| ------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 自动启用开发者模式、USB 调试、无线调试           | 普通应用不能直接写入相应 Global / Secure 设置；可以打开开发者选项，让用户操作。[Global 设置](https://developer.android.com/reference/android/provider/Settings.Global)、[Secure 设置](https://developer.android.com/reference/android/provider/Settings.Secure) |
| 自动批准新电脑的 ADB 调试授权                    | 标准 ADB 首次信任需要用户解锁并确认 RSA 授权框；打开页面不授予这项信任。[ADB 授权](https://developer.android.com/tools/adb)                                                                                                                                     |
| 静默开关 Wi-Fi                                   | target Android 10 及以上的普通应用调用 `setWifiEnabled` 会失败；可以打开 Wi-Fi 或网络面板。设备管理与系统应用另有例外。[Android 10 限制](https://developer.android.com/about/versions/10/privacy/changes)                                                       |
| 静默开关蓝牙                                     | target Android 13 及以上的普通应用调用 `BluetoothAdapter.enable / disable` 会失败；可以打开蓝牙设置。设备所有者、配置文件所有者及系统应用另有例外。[Android 13 限制](https://developer.android.com/about/versions/13/behavior-changes-13)                       |
| 静默授予运行时权限、特殊访问权，或开启无障碍服务 | 必须走对应的用户授权流程；跳转授权页不会自动取得权限。[运行时授权](https://developer.android.com/training/permissions/requesting)、[无障碍启用](https://developer.android.com/reference/android/accessibilityservice/AccessibilityService)                      |
| 无特殊访问权地操作任意其他应用界面               | 普通 Intent 只能调用对方公开入口；跨应用的节点操作与手势需要上述无障碍能力，或另行具备 ADB 等执行环境                                                                                                                                                           |

以上页面跳转按用户在前台点击判断。后台收到请求后自动弹出系统页还受 Android 的后台 Activity 启动限制，不能从前台可行推导为后台可行。[后台启动限制](https://developer.android.com/guide/components/activities/background-starts)
