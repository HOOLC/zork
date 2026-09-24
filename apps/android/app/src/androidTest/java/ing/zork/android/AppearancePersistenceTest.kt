package ing.zork.android

import android.app.Application
import androidx.lifecycle.ViewModelStore
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import kotlinx.coroutines.*
import kotlinx.coroutines.sync.Mutex
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AppearancePersistenceTest {
    @Test fun leavingThePageDoesNotCancelCommittedAppearance() = runBlocking {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val directory = context.noBackupFilesDir.resolve("appearance-persistence").apply { mkdirs() }
        val repo = ClientRepository(context, directory)
        repo.command("preferences", "theme" to "system")
        val models = ViewModelStore()
        val callerScope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
        lateinit var model: ClientViewModel
        withContext(Dispatchers.Main) {
            model = ClientViewModel(context.applicationContext as Application, repo)
            models.put("appearance", model)
        }
        val gate = ClientRepository::class.java.getDeclaredField("localGate").apply { isAccessible = true }.get(repo) as Mutex
        gate.lock()
        try {
            val caller = callerScope.launch { model.saveTheme("dark") }
            // Hold the store gate until the page's caller has been disposed.
            delay(100)
            caller.cancelAndJoin()
        } finally { gate.unlock() }
        try {
            withTimeout(3000) { while (model.theme != "dark") delay(20) }
            assertEquals("dark", repo.command("preferences").getString("theme"))
            withContext(Dispatchers.Main) {
                models.clear()
                model = ClientViewModel(context.applicationContext as Application, repo)
                models.put("appearance", model)
            }
            withTimeout(3000) { while (model.theme != "dark") delay(20) }
            assertEquals("dark", model.theme)
        } finally {
            callerScope.cancel()
            withContext(Dispatchers.Main) { models.clear() }
        }
    }
}
