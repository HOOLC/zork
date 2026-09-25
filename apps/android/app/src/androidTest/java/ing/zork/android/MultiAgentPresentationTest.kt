package ing.zork.android

import android.content.Intent
import android.os.SystemClock
import android.view.accessibility.AccessibilityNodeInfo
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Multi-agent transcript and chat list over core `messagePresentation` (fixture rows as observed). */
@RunWith(AndroidJUnit4::class)
class MultiAgentPresentationTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val automation = instrumentation.uiAutomation

    private fun nodes(): List<AccessibilityNodeInfo> {
        automation.clearCache()
        val out = mutableListOf<AccessibilityNodeInfo>()
        fun walk(n: AccessibilityNodeInfo?) {
            if (n == null) return
            if (n.isVisibleToUser) out += n
            for (i in 0 until n.childCount) walk(n.getChild(i))
        }
        walk(automation.rootInActiveWindow)
        return out
    }
    private fun label(n: AccessibilityNodeInfo) = listOfNotNull(n.text?.toString(), n.contentDescription?.toString()).joinToString(" ")
    private fun find(predicate: (String) -> Boolean) = nodes().firstOrNull { predicate(label(it)) }
    private fun await(what: String, predicate: (String) -> Boolean): AccessibilityNodeInfo {
        val end = SystemClock.uptimeMillis() + 6000
        while (SystemClock.uptimeMillis() < end) { find(predicate)?.let { return it }; Thread.sleep(50) }
        throw AssertionError("Missing $what: " + nodes().map(::label).filter { it.isNotBlank() })
    }
    private fun capture(name: String) {
        val folder = File(instrumentation.targetContext.filesDir, "multi-agent").apply { mkdirs() }
        automation.takeScreenshot()?.let { bitmap -> File(folder, name).outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG, 100, it) }; bitmap.recycle() }
    }
    private fun launch(screen: String, theme: String = "light") = ActivityScenario.launch<MultiAgentPreviewActivity>(
        Intent(instrumentation.targetContext, MultiAgentPreviewActivity::class.java).putExtra("screen", screen).putExtra("theme", theme))
    private fun swipeDown() {
        val size = instrumentation.targetContext.resources.displayMetrics
        instrumentation.uiAutomation.executeShellCommand("input swipe ${size.widthPixels / 2} ${size.heightPixels / 3} ${size.widthPixels / 2} ${size.heightPixels * 5 / 6} 300").close()
        Thread.sleep(900); instrumentation.waitForIdleSync()
    }

    /** Scrolls toward older rows until [predicate] matches a visible node. */
    private fun scrollTo(what: String, predicate: (String) -> Boolean): AccessibilityNodeInfo {
        repeat(8) { find(predicate)?.let { return it }; swipeDown() }
        return await(what, predicate)
    }

    @Test fun transcriptGroupsTimesAndReplyLinesComeFromCore() {
        launch("chat").use {
            Thread.sleep(600); instrumentation.waitForIdleSync()
            await("agent name") { it == "Builder" }
            await("relative time") { it == "刚刚" }
            // The last Builder reply answers the user's request with only its own
            // messages in between and the original within one screen: no line.
            assertNull("Adjacent reply line must be omitted", find { it.contains("回复") && it.contains("把这轮改动") })
            capture("transcript-tail.png")
            // A short original (≤ 10 chars) is quoted verbatim in 「」.
            scrollTo("verbatim short quote") { it.contains("「好。」") }
            // A sent comment batch: each passage as a quote line, then its reply.
            scrollTo("comment pair quote") { it.contains("「建议下移 8 px」") }
            await("comment pair reply") { it.contains("8 px 还是有点挤") }
            capture("transcript-pairs.png")
        }
    }

    @Test fun draftPassagesUseTheSentShape() {
        launch("drafts").use {
            Thread.sleep(600); instrumentation.waitForIdleSync()
            await("draft quote") { it.contains("「触控区重叠」") }
            await("draft reply field") { it.contains("回复这段") }
            await("optional extra text placeholder") { it.contains("补充说明（可选）") }
            assertNotNull("Removal is labelled", find { it.contains("移除这段引用") })
            capture("drafts.png")
        }
    }

    @Test fun chatListRowsHaveTitleThenMeta() {
        for (theme in listOf("light", "dark")) launch("list", theme).use {
            Thread.sleep(600); instrumentation.waitForIdleSync()
            await("title") { it == "登录页改版" }
            // Remote Chat: device letter · time; local Chat: time only.
            await("remote meta") { it.contains("在设备 A 上") && it.contains("刚刚") }
            await("local meta") { it.contains("10 分钟前") }
            assertNull("Local Chats show no device", find { it.contains("在设备 B 上") })
            capture("chat-list-$theme.png")
        }
    }
}
