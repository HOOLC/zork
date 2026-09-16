package surf.zork.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class DataResetHostTest {
    @Test fun rejectsUnconfirmedAndForeignRootsWithoutClearingTheApp() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        NativeBridge.initialize(context)
        val root = context.noBackupFilesDir.resolve("client").absolutePath
        val proof = context.filesDir.resolve("data-reset-proof").apply { writeText("keep until confirmed") }
        context.getSharedPreferences("data-reset-proof", 0).edit().putString("value", "keep").commit()
        val seeded = JSONObject(NativeBridge.call(root, "{\"op\":\"preferences\",\"message_preview_height\":360}"))
        assertTrue(seeded.optBoolean("ok"))
        assertFalse(JSONObject(NativeBridge.clearData(root, false, context)).optBoolean("ok"))
        assertFalse(JSONObject(NativeBridge.clearData(context.filesDir.absolutePath, true, context)).optBoolean("ok"))
        assertTrue(proof.exists())
        val preferences = JSONObject(NativeBridge.call(root, "{\"op\":\"preferences\"}"))
        assertEquals(360, preferences.getJSONObject("data").getInt("message_preview_height"))
    }
}
