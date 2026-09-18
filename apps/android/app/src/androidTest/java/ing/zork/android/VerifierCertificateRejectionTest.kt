package ing.zork.android

import android.content.Context
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class VerifierCertificateRejectionTest {
    private fun verify(cert: ByteArray, time: Long = System.currentTimeMillis()): Int {
        val clazz = Class.forName("org.rustls.platformverifier.CertificateVerifier")
        val method = clazz.getDeclaredMethod("verifyCertificateChain", Context::class.java,
            String::class.java, String::class.java, Array<String>::class.java,
            ByteArray::class.java, java.lang.Long.TYPE, Array<ByteArray>::class.java)
        method.isAccessible = true
        val result = method.invoke(null, InstrumentationRegistry.getInstrumentation().targetContext,
            (if (time == 0L) "notyetvalid.zork.test" else "untrusted.zork.test"), "RSA", arrayOf("1.3.6.1.5.5.7.3.1"), null, time, arrayOf(cert))!!
        return result.javaClass.getDeclaredField("code").apply { isAccessible = true }.getInt(result)
    }

    private fun untrusted() = InstrumentationRegistry.getInstrumentation().context.assets
        .open("tls/untrusted.der").use { it.readBytes() }

    @Test fun invalidDerStillFails() { assertEquals(5, verify(byteArrayOf(1, 2, 3))) }
    @Test fun notYetValidStillFails() { assertEquals(2, verify(untrusted(), 0L)) }
    @Test fun untrustedRootStillFailsOnRepeatedAttempts() {
        repeat(3) { assertEquals(3, verify(untrusted())) }
    }
}
