package ing.zork.android

import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.database.sqlite.SQLiteDatabase
import android.os.Build
import android.os.SystemClock
import androidx.lifecycle.Lifecycle
import androidx.test.core.app.ActivityScenario
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.FileInputStream

class NotificationDeliveryTest {
    @Test fun backgroundObserverPostsOnceAcknowledgesAndOpensTheCoreDestination() = runBlocking {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        assumeTrue("task-owned emulator only", Build.HARDWARE == "ranchu" && InstrumentationRegistry.getArguments().getString("notifications_fixture") == "true")
        val context = instrumentation.targetContext
        val manager = context.getSystemService(NotificationManager::class.java)
        assertFalse("Run on the fresh task emulator before granting notification permission", manager.areNotificationsEnabled())
        val root = context.noBackupFilesDir.resolve("client")
        val repo = ClientRepository(context)
        assertFalse("Requires fresh app data on the task-owned emulator", root.resolve("client.db").exists())
        root.mkdirs()
        val tag = "[\"zork-notification-v1\",\"notification-fixture\",[\"session\",\"notification-chat\"]]"
        val notice = JSONObject().put("id","notification-event").put("node","notification-fixture").put("session","notification-chat")
            .put("title","通知测试会话").put("kind","reply").put("created_at_ms",System.currentTimeMillis())
        val ledger = JSONObject().put("initialized",true).put("pending",JSONObject().put(tag,notice)).put("presented",JSONObject())
        // Finish the Android fixture writer before native SQLite opens the file.
        SQLiteDatabase.openOrCreateDatabase(root.resolve("client.db"),null).use { db ->
            db.execSQL("CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL)")
            db.execSQL("CREATE TABLE cache(node TEXT NOT NULL,key TEXT NOT NULL,value TEXT NOT NULL,PRIMARY KEY(node,key))")
            db.execSQL("INSERT OR REPLACE INTO nodes(id,value) VALUES(?,?)", arrayOf("notification-fixture",JSONObject().put("id","notification-fixture").put("name","通知测试设备").put("url","").put("local",false).toString()))
            db.execSQL("INSERT OR REPLACE INTO cache(node,key,value) VALUES(?,?,?)",arrayOf("device","network","{\"direct_only\":true}"))
            db.execSQL("INSERT OR REPLACE INTO cache(node,key,value) VALUES(?,?,?)",arrayOf("notification-fixture","notification-ledger-v1",ledger.toString()))
        }
        repo.command("snapshot")
        fun await(condition: () -> Boolean) {
            val deadline = SystemClock.uptimeMillis() + 12000
            while (!condition() && SystemClock.uptimeMillis() < deadline) Thread.sleep(80)
            assertTrue("Condition timed out", condition())
        }
        fun visibleText(label: String): Boolean {
            instrumentation.uiAutomation.clearCache()
            fun walk(node: android.view.accessibility.AccessibilityNodeInfo?): Boolean {
                if (node == null) return false
                if (node.text?.toString()?.contains(label) == true || node.contentDescription?.toString()?.contains(label) == true) return true
                return (0 until node.childCount).any { walk(node.getChild(it)) }
            }
            return walk(instrumentation.uiAutomation.rootInActiveWindow)
        }
        repo.command("notification_settings","operation" to JSONObject().put("action","background").put("value",true))
        try {
            ActivityScenario.launch<Nav7PreviewActivity>(Intent(context,Nav7PreviewActivity::class.java).putExtra("screen","home").putExtra("width",0)).use { activity ->
                activity.onActivity { NotificationPlatform.updateService(it,true) }
                await { ZorkNotificationService.running }
                activity.moveToState(Lifecycle.State.CREATED)
                instrumentation.uiAutomation.executeShellCommand("pm grant ${context.packageName} android.permission.POST_NOTIFICATIONS").use { fd ->
                    FileInputStream(fd.fileDescriptor).use { it.readBytes() }
                }
                // The service must consume this committed hint without any UI frame clock.
                repo.command("notification_settings","operation" to JSONObject().put("action","sound").put("value",false))
                await { manager.activeNotifications.any { it.tag == tag } }
                val notification = manager.activeNotifications.single { it.tag == tag }.notification
                assertEquals("Zork",notification.extras.getString("android.title"))
                assertEquals("收到新的回复",notification.extras.getString("android.text"))
                assertEquals("notification-event",notification.extras.getString("zork_event"))
                val destination = repo.command("open_notification","tag" to tag)
                assertEquals("notification-chat",destination.getString("session"))
                repo.command("notification_settings","operation" to JSONObject().put("action","sound").put("value",true))
                Thread.sleep(250)
                assertEquals(1,manager.activeNotifications.count { it.tag == tag })
                notification.contentIntent.send()
                await { visibleText("通知测试会话") }
                context.filesDir.resolve("notification-delivery.json").writeText(JSONObject().put("background_delivery",true)
                    .put("private_title",true).put("deduplicated",true).put("opened_session","notification-chat").toString(2))
            }
        } finally {
            repo.command("notification_settings","operation" to JSONObject().put("action","background").put("value",false))
            context.stopService(Intent(context,ZorkNotificationService::class.java))
            await { !ZorkNotificationService.running }
            manager.cancel(tag,NotificationPlatform.NOTICE_ID)
        }
    }
}
