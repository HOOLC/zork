package ing.zork.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.rustls.platformverifier.VerificationFailures

@RunWith(AndroidJUnit4::class)
class VerifierPowerTest {
    @Test fun repeatedFailureExpiresEvenIfContinuouslyRetried() {
        var time = 0L
        val failures = VerificationFailures<String, String>({ time }, 30L)
        failures.put("old-cert", "untrusted")
        time = 29L
        assertEquals("untrusted", failures.get("old-cert"))
        assertNull(failures.get("replacement-cert"))
        time = 30L
        assertNull(failures.get("old-cert"))
    }

    @Test fun explicitRetryCanClearTheCooldown() {
        val failures = VerificationFailures<String, String>({ 0L })
        failures.put("cert", "error")
        failures.clear()
        assertNull(failures.get("cert"))
    }

    @Test fun manyDistinctFailuresCannotGrowTheCacheWithoutBound() {
        val failures = VerificationFailures<String, String>({ 0L }, capacity = 2)
        failures.put("first", "error")
        failures.put("second", "error")
        failures.put("third", "error")
        assertNull(failures.get("first"))
        assertEquals("error", failures.get("second"))
        assertEquals("error", failures.get("third"))
    }
}
