package ing.zork.android

import android.app.Application
import android.database.sqlite.SQLiteDatabase
import android.graphics.Bitmap
import android.os.Build
import android.os.SystemClock
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.ui.Modifier
import androidx.lifecycle.ViewModelStore
import androidx.test.core.app.ActivityScenario
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.withTimeout
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import org.junit.runner.RunWith

/** The fixture is restricted to an explicitly selected task-owned emulator. */
@RunWith(AndroidJUnit4::class)
class ReplicaSettingsTest {
    @Test fun settingsReadCommittedReplicaWithoutNetworkAndSurviveViewModelRecreation() = runBlocking {
        assumeTrue("isolated emulator fixture only", Build.HARDWARE == "ranchu" &&
            InstrumentationRegistry.getArguments().getString("replica_fixture") == "true")
        val instrumentation = InstrumentationRegistry.getInstrumentation()
        val context = instrumentation.targetContext
        val root = context.filesDir.resolve("replica-settings-fixture").apply { deleteRecursively(); mkdirs() }
        val repo = ClientRepository(context, root)
        val peer = "replica-settings-fixture"
        val scope = JSONObject().put("type", "catalog").toString()
        val cursor = JSONObject().put("owner", "fixture-owner").put("epoch", "fixture-epoch")
            .put("scope", JSONObject(scope)).put("sequence", 3)
        val database = SQLiteDatabase.openOrCreateDatabase(root.resolve("client.db"), null)
        database.execSQL("CREATE TABLE nodes(id TEXT PRIMARY KEY,value TEXT NOT NULL)")
        database.execSQL("CREATE TABLE replica_scopes(peer TEXT NOT NULL,scope TEXT NOT NULL,cursor TEXT NOT NULL,last_batch TEXT NOT NULL,PRIMARY KEY(peer,scope))")
        database.execSQL("CREATE TABLE replica_entities(peer TEXT NOT NULL,scope TEXT NOT NULL,kind TEXT NOT NULL,id TEXT NOT NULL,revision INTEGER NOT NULL,value TEXT,PRIMARY KEY(peer,scope,kind,id))")
        database.beginTransaction()
        try {
            database.execSQL("INSERT OR REPLACE INTO nodes(id,value) VALUES(?,?)", arrayOf(peer,
                JSONObject().put("id", peer).put("name", "离线工作站").put("url", "").put("local", false).toString()))
            database.execSQL("INSERT OR REPLACE INTO replica_scopes(peer,scope,cursor,last_batch) VALUES(?,?,?,'fixture')", arrayOf(peer,scope,cursor.toString()))
            fun entity(kind: String, id: String, revision: Int, value: JSONObject) {
                database.execSQL("INSERT OR REPLACE INTO replica_entities(peer,scope,kind,id,revision,value) VALUES(?,?,?,?,?,?)", arrayOf(peer,scope,kind,id,revision,value.toString()))
            }
            entity("device", "self", 1, JSONObject().put("name", "离线工作站").put("gateway",JSONObject().put("version","fixture")))
            entity("agent", "offline-agent", 2, JSONObject().put("id","offline-agent").put("name","离线领队").put("role","leader").put("avatar","fox").put("profile_id","offline-profile").put("model","cached-model"))
            entity("profile", "offline-profile", 3, JSONObject().put("profile_id","offline-profile").put("name","已保存的模型连接").put("provider","openai").put("billing","subscription").put("models",JSONArray().put(JSONObject().put("id","cached-model"))))
            entity("provider", "openai", 3, JSONObject().put("id","openai").put("label","OpenAI").put("billing",JSONArray()))
            database.setTransactionSuccessful()
        } finally { database.endTransaction(); database.close() }
        repo.command("snapshot") // native schema migration starts only after fixture writer closes
        val measureLatency = InstrumentationRegistry.getArguments().getString("measure_latency") == "true"
        val measurements = mutableListOf<Long>()
        try {
            val gateField = ClientRepository::class.java.getDeclaredField("networkGate").apply { isAccessible = true }
            val networkGate = gateField.get(repo) as Mutex
            networkGate.lock()
            try {
                repeat(if(measureLatency) 20 else 1) {
                    val start = SystemClock.elapsedRealtimeNanos()
                    val snapshot = withTimeout(2000) { repo.command("settings", "peer" to peer, "cached_only" to true) }
                    if(measureLatency) measurements += (SystemClock.elapsedRealtimeNanos() - start) / 1_000_000
                    assertTrue(snapshot.getBoolean("ready"))
                    assertEquals("offline-profile",snapshot.getJSONArray("profiles").getJSONObject(0).getString("profile_id"))
                    assertEquals("OpenAI",snapshot.getJSONArray("providers").getJSONObject(0).getString("label"))
                }
            } finally { networkGate.unlock() }
            if(measureLatency) assertTrue("cached settings exceeded 100 ms: $measurements", measurements.max() < 100)
            val models = ViewModelStore()
            ActivityScenario.launch(Nav7PreviewActivity::class.java).use { scenario ->
                repeat(2) { generation ->
                    lateinit var model: ClientViewModel
                    scenario.onActivity { activity ->
                        models.clear()
                        model = ClientViewModel(context.applicationContext as Application, repo)
                        models.put("settings",model)
                        activity.setContent { ZorkTheme { model.settings?.let { MobileSettings(it,emptyList(),SettingsActions(),Modifier.safeDrawingPadding()) } } }
                        model.showDevice(Peer(peer,"离线工作站",""))
                        model.settingsPage("models")
                    }
                    val deadline=SystemClock.elapsedRealtime()+5000
                    var ready=false
                    while(!ready && SystemClock.elapsedRealtime()<deadline) {
                        scenario.onActivity { ready=model.settings?.profiles?.any { it.text("profile_id")=="offline-profile" } == true && model.settings?.loading == false }
                        if(!ready) Thread.sleep(20)
                    }
                    assertTrue("cached model catalog not shown after reopen $generation",ready)
                    scenario.onActivity { assertFalse(model.settings!!.online);assertEquals("离线领队",model.settings!!.agents.single().text("name")) }
                    instrumentation.waitForIdleSync()
                    val image=instrumentation.uiAutomation.takeScreenshot()
                    assertNotNull(image)
                    context.filesDir.resolve("replica-settings-$generation.png").outputStream().use { image.compress(Bitmap.CompressFormat.PNG,100,it) }
                    image.recycle()
                }
                scenario.onActivity { models.clear() }
            }
            context.filesDir.resolve("replica-settings.json").writeText(JSONObject().put("cached_ms",JSONArray(measurements)).put("latency_measured",measureLatency).put("network_gate_blocked",true).put("view_model_reopens",2).toString())
        } finally {
            // The private fixture is retained for diagnosis. Never reopen its DB
            // through Android SQLite while the native store is still alive.
        }
    }
}
