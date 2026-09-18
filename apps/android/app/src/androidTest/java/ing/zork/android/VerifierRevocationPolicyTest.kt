package ing.zork.android

import androidx.test.ext.junit.runners.AndroidJUnit4
import java.security.cert.PKIXRevocationChecker.Option
import org.junit.Assert.*
import org.junit.Test
import org.junit.runner.RunWith
import org.rustls.platformverifier.CertificateVerifier

@RunWith(AndroidJUnit4::class)
class VerifierRevocationPolicyTest {
    @Test fun untrustedCertificateIsStillRejected() {
        val context = androidx.test.platform.app.InstrumentationRegistry.getInstrumentation().context
        val der = context.assets.open("untrusted-server.der").use { it.readBytes() }
        val method = CertificateVerifier::class.java.declaredMethods.single { it.name == "verifyCertificateChain" }
        method.isAccessible = true
        val result = method.invoke(null, context, "untrusted.invalid", "RSA",
            arrayOf("1.3.6.1.5.5.7.3.1"), null, System.currentTimeMillis(), arrayOf(der))
        val code = result.javaClass.getDeclaredField("code").apply { isAccessible = true }.getInt(result)
        assertNotEquals("An untrusted self-signed certificate must not validate", 0, code)
    }

    @Test fun crlOnlyCertificateUsesItsAdvertisedRevocationMechanism() {
        val policy = CertificateVerifier.revocationOptions(false, null, byteArrayOf(0x04, 0x00))
        assertTrue(policy.contains(Option.PREFER_CRLS))
        assertTrue(policy.contains(Option.NO_FALLBACK))
        assertTrue(policy.contains(Option.ONLY_END_ENTITY))
    }

    @Test fun explicitStapledOcspIsNeverBypassed() {
        val policy = CertificateVerifier.revocationOptions(true, null, byteArrayOf(0x04, 0x00))
        assertFalse(policy.contains(Option.PREFER_CRLS))
        assertFalse(policy.contains(Option.NO_FALLBACK))
    }

    @Test fun advertisedOcspOrMissingCrlRetainsUpstreamPolicy() {
        val aia = byteArrayOf(0x04, 0x0c, 0x30, 0x0a, 0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01)
        assertFalse(CertificateVerifier.revocationOptions(false, aia, byteArrayOf(0x04, 0x00)).contains(Option.PREFER_CRLS))
        assertFalse(CertificateVerifier.revocationOptions(false, null, null).contains(Option.NO_FALLBACK))
    }
}
