package ing.zork.android

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/** Fixtures assemble the same complete controls used by production screens. */
class LiquidGalleryActivity : ComponentActivity() {
    internal var host: LiquidSceneHost? = null
        private set
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent { ZorkTheme {
            val scene = LocalLiquidHost.current
            SideEffect { host = scene }
            var selected by remember { mutableStateOf("day") }
            var checked by remember { mutableStateOf(false) }
            var radio by remember { mutableStateOf("local") }
            var slider by remember { mutableFloatStateOf(.35f) }
            var value by remember { mutableStateOf("") }
            var count by remember { mutableIntStateOf(0) }
            var overlays by remember { mutableStateOf(false) }
            var dialog by remember { mutableStateOf(false) }
            var sheet by remember { mutableStateOf(false) }
            var disclosure by remember { mutableStateOf(false) }
            var menu by remember { mutableStateOf("local") }
            LiquidDialog(dialog, "编辑名称", { dialog = false }) {
                Text("编辑名称", fontSize = 20.sp)
                SettingsField("弹窗名称", value, { value = it })
                SettingsButton("完成编辑", primary = true) { dialog = false }
            }
            if (sheet) SettingsSheet("更多设置", dismiss = { sheet = false }) {
                SettingsField("面板名称", value, { value = it })
                SettingsToggle("保留离线副本", checked) { checked = it }
            }
            Column(Modifier.fillMaxSize().background(ZorkColors.Paper).statusBarsPadding().navigationBarsPadding()
                .verticalScroll(rememberScrollState()).padding(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                Text("液态组件", fontSize = 24.sp)
                LiquidButton(if (overlays) "返回基础控件" else "浮层与展开", onClick = { overlays = !overlays })
                if (overlays) {
                    SettingsSelect("同步范围", menu, listOf("local" to "当前设备", "all" to "所有设备")) { menu = it }
                    SettingsButton("打开对话框") { dialog = true }
                    SettingsListGroup { SettingsListRow("打开底部面板", action = { sheet = true }) }
                    LiquidDisclosure("高级选项", disclosure, { disclosure = it }) {
                        SettingsField("备注", value, { value = it })
                        SettingsToggle("显示提示", checked) { checked = it }
                    }
                } else {
                LiquidCard(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                        Text("按钮与选择", fontSize = 17.sp)
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            LiquidButton("主操作", primary = true, onClick = { count++ })
                            LiquidButton("次操作", onClick = { count++ })
                        }
                        Text("已触发 $count 次", fontSize = 12.sp, color = ZorkColors.Muted)
                        SettingsSegments(listOf("day" to "日视图", "week" to "周视图", "month" to "月视图"), selected) { selected = it }
                        LiquidCheckbox("保留离线副本", checked, { checked = it })
                        SettingsToggle("启用同步", checked) { checked = it }
                        LiquidRadioGroup(listOf("local" to "仅此设备", "all" to "全部设备"), radio, { radio = it })
                        LiquidSlider("缩放比例", slider, { slider = it })
                        LiquidProgress(slider)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            LiquidBadge("已保存")
                            LiquidBadge("进行中", emphasized = true)
                        }
                    }
                }
                LiquidCard(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                        Text("原生文字输入", fontSize = 17.sp)
                        SettingsField("名称", value, { value = it }, detail = "支持中文、选择与粘贴")
                        SettingsField("只读预览", "共享轮廓", {}, enabled = false)
                        SettingsButton("不可用操作", primary = true, enabled = false) { }
                    }
                }
                }
            }
        } }
    }
}
