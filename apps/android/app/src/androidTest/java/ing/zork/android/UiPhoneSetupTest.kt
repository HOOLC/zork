package ing.zork.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/** Explicitly invoked for the user's requested fresh-phone UI installation.
 * Does not clear app data or alter an existing peer. Target is an isolated lab. */
@RunWith(AndroidJUnit4::class)
class UiPhoneSetupTest {
    @Test fun connectIsolatedUiFixture() {
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val args = InstrumentationRegistry.getArguments()
        val root = context.noBackupFilesDir.resolve("client").absolutePath
        NativeBridge.initialize(context)
        fun call(request: JSONObject): JSONObject {
            val response = JSONObject(NativeBridge.call(root,request.toString()))
            assertTrue(response.toString(),response.getBoolean("ok"))
            return response.getJSONObject("data")
        }
        var state = call(JSONObject().put("op","resume"))
        val origin = args.getString("origin")
        if (origin != null) {
            state = call(JSONObject().put("op","save_peer").put("origin",origin)
                .put("name","界面验证 · 测试设备").put("address",requireNotNull(args.getString("address"))))
            context.getSharedPreferences("navigation",0).edit().putString("peer",origin).commit()
        }
        File(context.filesDir,"ui-phone-identity.json").writeText(JSONObject().put("identity",state.getString("identity")).toString())
        call(JSONObject().put("op","pause"))
    }
}
