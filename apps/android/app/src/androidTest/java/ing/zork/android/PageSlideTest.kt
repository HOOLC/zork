package ing.zork.android

import android.content.Intent
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class PageSlideTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private fun settle() { Thread.sleep(400); instrumentation.waitForIdleSync() }
    @Test fun captureDeviceEntry() {
        if (InstrumentationRegistry.getArguments().getString("capture_device_motion") != "true") return
        ActivityScenario.launch<PageSlideActivity>(Intent(instrumentation.targetContext, PageSlideActivity::class.java)).use { scenario ->
            settle()
            fun shot(name: String) {
                instrumentation.uiAutomation.takeScreenshot().let { bitmap ->
                    instrumentation.targetContext.filesDir.resolve("device-$name.png").outputStream().use { bitmap.compress(android.graphics.Bitmap.CompressFormat.PNG,100,it) }
                }
            }
            shot("before")
            scenario.onActivity { it.go("device") }
            repeat(4) { Thread.sleep(100); shot("during-$it") }
            Thread.sleep(1400);shot("after")
            scenario.onActivity { it.refresh() };settle();shot("loading")
            scenario.onActivity { it.refresh() };settle();shot("ready")
        }
    }
    @Test fun routesUseNavigationDepthNotLoadingState() {
        val device = MobileSettingsState("device", Peer("a", "a", ""))
        assertEquals(settingsRouteKey(device), settingsRouteKey(device.copy(loading=true)))
        assertTrue(settingsRouteDepth(device.copy(page="models")) > settingsRouteDepth(device))
        assertTrue(settingsRouteDepth(device.copy(page="profile")) > settingsRouteDepth(device.copy(page="models")))
        assertEquals(1, settingsRouteDepth(device.copy(fromChat=true)))
        val connections = MobileSettingsState("model-connections")
        assertEquals(2, settingsRouteDepth(connections))
        assertTrue(settingsRouteDepth(device.copy(page="profile", fromConnections=true)) > settingsRouteDepth(connections))
        assertEquals(settingsRouteDepth(device.copy(page="models", fromConnections=true)), settingsRouteDepth(device.copy(page="profile", fromConnections=true)))
        assertNotEquals(settingsRouteKey(device), settingsRouteKey(device.copy(device=Peer("b","b",""))))
    }
    @Test fun forwardBackRefreshAndInterruptedNavigation() {
        ActivityScenario.launch<PageSlideActivity>(Intent(instrumentation.targetContext, PageSlideActivity::class.java).putExtra("trackMotion", true)).use { scenario ->
            settle()
            var origin=0f
            scenario.onActivity { origin=it.positions.last(); it.positions.clear(); it.go("device") }
            settle()
            scenario.onActivity {
                assertTrue("Forward page must come from the right: ${it.positions}",it.positions.any { x -> x > origin+2 })
                assertEquals(origin,it.positions.last(),1f)
                assertTrue("Old page must survive until new page is mounted: ${it.pageLifetimes}",
                    it.pageLifetimes.indexOf("enter:device") < it.pageLifetimes.indexOf("exit:home"))
                it.positions.clear();it.outgoingPositions.clear();it.go("home")
            }
            settle()
            scenario.onActivity {
                assertTrue("Returning page must come from the left: ${it.positions}",it.positions.any { x -> x < origin-2 })
                assertEquals(origin,it.positions.last(),1f)
                val outgoing = it.outgoingPositions.toList()
                assertTrue("Must observe outgoing page: $outgoing", outgoing.size >= 3)
                assertTrue("Outgoing page starts on screen: $outgoing", outgoing.first() < 100f)
                assertTrue("Outgoing page slides right, without reversing: $outgoing", outgoing.zipWithNext().all { (a,b) -> b >= a - 1f })
                assertTrue("Outgoing page must travel off to the right: $outgoing", outgoing.last() - outgoing.first() > 250f)
                it.positions.clear();it.refresh()
            }
            settle()
            scenario.onActivity { assertTrue("Refreshing must not replay navigation",it.positions.all { x -> kotlin.math.abs(x-origin)<1 });it.go("device") }
            Thread.sleep(40)
            scenario.onActivity { it.go("home") }
            Thread.sleep(40)
            scenario.onActivity { it.go("about") }
            settle()
            scenario.onActivity { assertEquals("about",it.page.page);assertEquals(origin,it.positions.last(),1f) }
        }
    }
    @Test fun measureSettingsTransitions() {
        for (animated in listOf(false,true)) {
            ActivityScenario.launch<PageSlideActivity>(Intent(instrumentation.targetContext, PageSlideActivity::class.java).putExtra("animated",animated)).use { scenario ->
                settle(); scenario.onActivity { synchronized(it.frameTimes) { it.frameTimes.clear() } }
                for (route in listOf("device","models","device","models","profile","models","device","home","diagnostics","home","about","home")) {
                    scenario.onActivity { it.go(route) };settle()
                }
                scenario.onActivity {
                    val samples=synchronized(it.frameTimes){it.frameTimes.sorted()}
                    assertTrue(samples.isNotEmpty())
                    val result=JSONObject().put("animated",animated).put("frames",samples.size)
                        .put("layout_draw_p95_ms",samples[((samples.size-1)*.95).toInt()])
                        .put("layout_draw_p99_ms",samples[((samples.size-1)*.99).toInt()])
                    instrumentation.targetContext.filesDir.resolve("settings-transition-$animated.json").writeText(result.toString())
                }
            }
        }
    }
}
