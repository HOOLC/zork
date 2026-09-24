package ing.zork.android

import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Rect
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Test
import org.junit.Assert.*
import org.junit.runner.RunWith
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

@RunWith(AndroidJUnit4::class)
class SettingsRefreshTest {
    @Test fun settingsScreensAndRename() {
        val instrumentation=InstrumentationRegistry.getInstrumentation()
        val context=instrumentation.targetContext
        val folder=File(context.filesDir,"settings-refresh").apply{mkdirs()}
        fun find(label:String):AccessibilityNodeInfo? {
            instrumentation.uiAutomation.clearCache()
            fun walk(node:AccessibilityNodeInfo?):AccessibilityNodeInfo? { if(node==null)return null;if(node.text?.toString()?.lineSequence()?.any { it == label } == true || node.contentDescription?.toString()==label)return node;for(i in 0 until node.childCount){walk(node.getChild(i))?.let{return it}};return null }
            return walk(instrumentation.uiAutomation.rootInActiveWindow)
        }
        fun click(label:String) {
            var node=find(label);val end=System.currentTimeMillis()+5000
            while(node==null && System.currentTimeMillis()<end){Thread.sleep(80);node=find(label)}
            assertNotNull(label,node)
            while(node!=null && !node.isClickable)node=node.parent
            assertTrue(label,node!!.performAction(AccessibilityNodeInfo.ACTION_CLICK));instrumentation.waitForIdleSync();Thread.sleep(250)
        }
        for(width in if(InstrumentationRegistry.getArguments().getString("native_only")=="true") listOf(0) else listOf(320,375,414,768,0))for(screen in listOf("device","new-chat","models","profile")) {
            ActivityScenario.launch<Nav7PreviewActivity>(Intent(context,Nav7PreviewActivity::class.java).putExtra("screen",screen).putExtra("width",width)).use { scenario ->
                instrumentation.waitForIdleSync();Thread.sleep(450)
                fun capture(name:String) {
                    if(width==0) {
                        Thread.sleep(1500)
                        val screenshot=instrumentation.uiAutomation.takeScreenshot()
                        assertNotNull("Active dialog screenshot",screenshot)
                        File(folder,"$name-$width.png").outputStream().use{screenshot.compress(Bitmap.CompressFormat.PNG,100,it)}
                        screenshot.recycle();return
                    }
                    var bounds=Rect();scenario.onActivity{bounds=Rect(it.contentBounds)}
                    assertEquals(width,bounds.width());assertEquals(844,bounds.height())
                    val bitmap=Bitmap.createBitmap(width,844,Bitmap.Config.ARGB_8888);val ready=CountDownLatch(1);var result=-1
                    scenario.onActivity { android.view.PixelCopy.request(it.window,bounds,bitmap,{result=it;ready.countDown()},android.os.Handler(android.os.Looper.getMainLooper())) }
                    assertTrue(ready.await(5,TimeUnit.SECONDS));assertEquals(android.view.PixelCopy.SUCCESS,result)
                    File(folder,"$name-$width.png").outputStream().use{bitmap.compress(Bitmap.CompressFormat.PNG,100,it)};bitmap.recycle()
                }
                if(screen=="device") { assertNull(find("运行方式"));assertNull(find("后台运行"));assertNull(find("检查更新"));assertNull(find("更新说明")) }
                if(screen=="profile") assertNull(find("从提供商获取模型"))
                capture(screen)
                if(width==0 && screen=="device") {
                    click("更多");click("重命名");capture("rename")
                    click("保存");scenario.onActivity{assertEquals("rename_device",it.lastAction)}
                }
                if(width==0 && screen=="models") {click("添加");capture("connection-editor")}
                if(width==0 && screen=="profile") {click("手动添加");capture("model-editor")}
            }
        }
    }
}
