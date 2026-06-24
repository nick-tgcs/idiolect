package org.idiolect.android.sync

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test
import java.security.cert.CertificateException
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate

/**
 * The phone side of pin-on-pairing: trusting the PC's self-signed sync cert by its SPKI
 * fingerprint alone (no CA, no hostname). Host-tested with a fixed self-signed fixture in
 * `src/test/resources/pinning` whose canonical pin ([PIN]) was computed by
 * `openssl x509 -pubkey | pkey -pubin -outform der | dgst -sha256` — exactly the SPKI hash the
 * Rust server publishes in the QR. The cross-language contract is asserted directly by
 * [the_fingerprint_matches_the_openssl_canonical_spki_hash].
 */
class CertPinTest {
    private fun fixtureCert(): X509Certificate {
        val stream = javaClass.getResourceAsStream("/pinning/server.crt")
            ?: error("missing /pinning/server.crt fixture")
        return stream.use {
            CertificateFactory.getInstance("X.509").generateCertificate(it) as X509Certificate
        }
    }

    @Test
    fun the_fingerprint_matches_the_openssl_canonical_spki_hash() {
        // Our `sha256(cert.publicKey.encoded)` must equal what openssl hashed from the same
        // cert's SPKI — which is what the Rust server hashes from its key pair. If these ever
        // diverged, pinning would reject every real device.
        assertEquals(PIN, CertPin.sha256Hex(fixtureCert().publicKey.encoded))
    }

    @Test
    fun a_matching_pin_trusts_the_presented_cert() {
        // No exception thrown == trusted.
        CertPin(PIN).trustManager().checkServerTrusted(arrayOf(fixtureCert()), "ECDHE_ECDSA")
    }

    @Test
    fun a_matching_pin_is_case_insensitive() {
        CertPin(PIN.uppercase()).trustManager().checkServerTrusted(arrayOf(fixtureCert()), "ECDHE_ECDSA")
    }

    @Test
    fun a_wrong_pin_rejects_the_presented_cert() {
        // A man-in-the-middle presenting any other cert fails the pin → the handshake aborts.
        assertThrows(CertificateException::class.java) {
            CertPin("0".repeat(64)).trustManager().checkServerTrusted(arrayOf(fixtureCert()), "ECDHE_ECDSA")
        }
    }

    @Test
    fun an_empty_chain_is_rejected() {
        assertThrows(CertificateException::class.java) {
            CertPin(PIN).trustManager().checkServerTrusted(emptyArray(), "ECDHE_ECDSA")
        }
    }

    companion object {
        /** sha256 of the fixture cert's DER SubjectPublicKeyInfo (src/test/resources/pinning). */
        private const val PIN = "35e1083863409f0a52912f562f973a0c56eb050d638672b25474069dd4ae6e00"
    }
}
