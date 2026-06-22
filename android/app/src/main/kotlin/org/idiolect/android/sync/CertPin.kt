package org.idiolect.android.sync

import java.net.HttpURLConnection
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.HostnameVerifier
import javax.net.ssl.HttpsURLConnection
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLSocketFactory
import javax.net.ssl.TrustManager
import javax.net.ssl.X509TrustManager

/**
 * Certificate pinning for the PC's self-signed sync server — the trust anchor for the default
 * HTTPS transport. The server is reached by bare IP, with no CA and no hostname to verify, so
 * trust rests entirely on the SPKI fingerprint the pairing QR delivered out-of-band (`f=`): we
 * trust the presented cert iff the SHA-256 of its DER `SubjectPublicKeyInfo`
 * (`X509Certificate.publicKey.encoded`) equals that pin. That is the exact mirror of the Rust
 * server's `sha256(key_pair.public_key_der())` and the `pairing_over_https` integration-test
 * verifier — the three compute the same primitive, asserted on each side against a shared
 * literal.
 *
 * Dependency-free (`javax.net.ssl` + `java.security` only — no OkHttp), keeping the APK lean
 * and FOSS for GrapheneOS, matching the `HttpURLConnection` transports it equips.
 */
class CertPin(fingerprint: String) {
    private val pin = fingerprint.lowercase()

    /** A trust manager that trusts a server chain iff its leaf's SPKI hashes to the pin. */
    fun trustManager(): X509TrustManager = object : X509TrustManager {
        override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
            val leaf = chain?.firstOrNull()
                ?: throw CertificateException("no server certificate presented")
            val presented = sha256Hex(leaf.publicKey.encoded)
            if (presented != pin) {
                throw CertificateException("certificate pin mismatch (expected $pin, got $presented)")
            }
        }

        // The phone is a TLS *client*; it never validates client certs.
        override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) {
            throw CertificateException("client authentication is not supported")
        }

        override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
    }

    /** An [SSLSocketFactory] whose only trust anchor is the pin. */
    fun socketFactory(): SSLSocketFactory {
        val context = SSLContext.getInstance("TLS")
        context.init(null, arrayOf<TrustManager>(trustManager()), null)
        return context.socketFactory
    }

    companion object {
        /**
         * We pin the SPKI, not the hostname — the server is IP-addressed and its self-signed
         * cert's SAN is cosmetic — so hostname verification is intentionally a no-op. The pin
         * is the whole identity.
         */
        val ACCEPT_ANY_HOSTNAME = HostnameVerifier { _, _ -> true }

        /** Lowercase-hex SHA-256, matching the `f=` fingerprint the server publishes. */
        fun sha256Hex(bytes: ByteArray): String =
            MessageDigest.getInstance("SHA-256").digest(bytes)
                .joinToString("") { "%02x".format(it.toInt() and 0xFF) }
    }
}

/**
 * Equip [connection] for the pinned-TLS default before it connects. An `https://` PC endpoint
 * **requires** a [pin]: we trust only the cert whose SPKI matches it (no CA, no hostname). A
 * cleartext `http://` endpoint (the `--no-tls` fallback) is left untouched. An `https` endpoint
 * with no pin is a misconfiguration and throws, rather than silently leaning on system trust —
 * which would reject the self-signed server anyway, but failing loudly is clearer. Shared by
 * every PC transport ([HttpPairingTransport], [HttpSyncTransport], `HttpModelTransport`) so
 * pairing, sync, and the model pull all pin identically.
 */
fun applyPinning(connection: HttpURLConnection, pin: String?) {
    if (connection is HttpsURLConnection) {
        requireNotNull(pin) { "an https PC endpoint requires a pinned certificate fingerprint" }
        connection.sslSocketFactory = CertPin(pin).socketFactory()
        connection.hostnameVerifier = CertPin.ACCEPT_ANY_HOSTNAME
    }
}
