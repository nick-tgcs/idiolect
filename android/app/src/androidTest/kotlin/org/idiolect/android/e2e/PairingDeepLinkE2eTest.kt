package org.idiolect.android.e2e

import android.content.Intent
import android.net.Uri
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.UiDevice
import org.idiolect.android.settings.SettingsActivity
import org.idiolect.android.sync.HttpPairingTransport
import org.idiolect.android.sync.PairingTokenStore
import org.idiolect.android.sync.SecureSyncConfig
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.OutputStream
import java.net.Socket
import java.net.URLEncoder
import java.security.KeyStore
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLServerSocket
import kotlin.concurrent.thread

/**
 * End-to-end on the emulator, camera-free, over the **default pinned-TLS** transport: fires the
 * real `idiolect://pair?u=…&c=…&f=…` deep link (the same URI the PC's pairing QR encodes under
 * TLS) and asserts the whole Android pairing chain runs on a real device — the OS routes the
 * link to [SetupActivity], which parses it, POSTs the code over a real pinned `HttpsURLConnection`
 * (trusting only the cert whose SPKI matches the `f=` pin), and persists the issued token
 * **wrapped by the real AndroidKeyStore** plus the pin, reloadable via [SecureSyncConfig].
 *
 * The PC is stood up as a tiny in-process **HTTPS** server on the device's loopback, presenting
 * the self-signed fixture cert (`androidTest/assets/pinning/server.p12`) whose SPKI hashes to
 * [PIN] — the same fixture and pin the host `CertPinTest` / `HttpPairingTransportTlsTest` use, and
 * the same pin-on-pairing contract the Rust `pairing_over_https.rs` proves on the server side. So
 * the test is hermetic — no host server, no network, no CA. A matching pin pairs and persists; a
 * wrong pin aborts the handshake on-device (Conscrypt), so the stub never even sees the request.
 */
@RunWith(AndroidJUnit4::class)
class PairingDeepLinkE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)
    private val targetContext get() = instrumentation.targetContext

    private lateinit var server: SSLServerSocket
    private var port = 0

    /** The request line the stub PC server last saw — distinguishes "the link never reached
     *  the server" (routing / a failed pin) from "reached it but didn't persist" (parse/keystore). */
    private val lastRequest = AtomicReference("<none>")

    @Before
    fun startPcStubAndClearPairing() {
        // A fresh pairing: drop any persisted endpoint so we observe *this* enrolment.
        File(targetContext.filesDir, SecureSyncConfig.URL_FILE_NAME).delete()
        File(targetContext.filesDir, SecureSyncConfig.PIN_FILE_NAME).delete()
        File(targetContext.filesDir, PairingTokenStore.FILE_NAME).delete()

        // A real TLS endpoint presenting the self-signed fixture cert (bundled in the test
        // APK's assets), so the on-device handshake is genuine and the phone must pin it.
        val keyStore = KeyStore.getInstance("PKCS12")
        instrumentation.context.assets.open("pinning/server.p12").use { keyStore.load(it, PASSWORD) }
        val kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm())
        kmf.init(keyStore, PASSWORD)
        val context = SSLContext.getInstance("TLS")
        context.init(kmf.keyManagers, null, null)
        server = context.serverSocketFactory.createServerSocket(0) as SSLServerSocket
        port = server.localPort
        thread(isDaemon = true) {
            while (!server.isClosed) {
                val socket = try {
                    server.accept()
                } catch (_: Exception) {
                    break
                }
                try {
                    handle(socket)
                } catch (_: Exception) {
                    // teardown, a dropped model pull, or a client that aborted on a pin mismatch
                }
            }
        }
    }

    @After
    fun stop() {
        server.close()
        device.pressHome()
    }

    @Test
    fun the_pinned_transport_reaches_a_loopback_pc_over_tls() {
        // The on-device counterpart to HttpPairingTransportTlsTest: the real HttpsURLConnection
        // completes a *pinned* handshake against a loopback PC at runtime (Conscrypt honours the
        // pin), independent of the activity/deep-link wiring.
        val response = HttpPairingTransport("https://127.0.0.1:$port", pin = PIN).requestToken(CODE, "diag")
        assertEquals("the PC stub got the pair POST over TLS", "POST /v1/pair", lastRequest.get())
        assertEquals(TOKEN, response.token)
    }

    @Test
    fun a_pinned_pairing_deep_link_pairs_and_persists_token_and_pin() {
        val baseUrl = "https://127.0.0.1:$port"
        val uri = "idiolect://pair?u=${URLEncoder.encode(baseUrl, "UTF-8")}&c=$CODE&f=$PIN"

        // Fire it the way the OS would for a tapped link — an *implicit* VIEW + BROWSABLE the
        // manifest's deep-link filter resolves to SetupActivity. Going through a Uri object (not
        // `am start`) keeps the '&' separators intact, which a shell would split.
        targetContext.startActivity(
            Intent(Intent.ACTION_VIEW, Uri.parse(uri))
                .addCategory(Intent.CATEGORY_BROWSABLE)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
        )

        // The pairing runs off the UI thread; poll the real keystore-backed config until the
        // issued token lands (same process + filesDir as the app under instrumentation).
        val config = SecureSyncConfig.keystoreBacked(targetContext.filesDir)
        var settings = config.load()
        var waited = 0L
        while (settings?.token != TOKEN && waited < TIMEOUT_MS) {
            Thread.sleep(POLL_MS)
            waited += POLL_MS
            settings = config.load()
        }

        assertNotNull(
            "the deep link never persisted a paired endpoint (timed out); " +
                "last request the PC stub saw: ${lastRequest.get()}",
            settings,
        )
        assertEquals("the persisted token is the one the PC issued", TOKEN, settings!!.token)
        assertEquals("the persisted endpoint is the deep link's base URL", baseUrl, settings.baseUrl)
        assertEquals("the scanned pin is persisted so later syncs re-pin", PIN, settings.pin)
    }

    @Test
    fun a_wrong_pin_deep_link_never_pairs_and_never_reaches_the_pc() {
        val baseUrl = "https://127.0.0.1:$port"
        val wrongPin = "0".repeat(64)
        val uri = "idiolect://pair?u=${URLEncoder.encode(baseUrl, "UTF-8")}&c=$CODE&f=$wrongPin"

        targetContext.startActivity(
            Intent(Intent.ACTION_VIEW, Uri.parse(uri))
                .addCategory(Intent.CATEGORY_BROWSABLE)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
        )

        // The pin won't match the presented cert, so the TLS handshake aborts before the POST.
        // Give the background pairing ample time to try and fail, then assert nothing happened.
        Thread.sleep(NEGATIVE_WAIT_MS)
        assertNull(
            "a wrong pin must abort the handshake, so no endpoint is ever persisted",
            SecureSyncConfig.keystoreBacked(targetContext.filesDir).load(),
        )
        assertEquals(
            "the pair POST must never cross a failed-pin handshake",
            "<none>",
            lastRequest.get(),
        )
    }

    @Test
    fun a_pairing_link_into_settings_repairs_over_the_pinned_endpoint() {
        // The re-pair path: once a device is already paired, SetupActivity forwards an
        // idiolect://pair link to SettingsActivity (PairingRouter). Here we drive SettingsActivity
        // directly with that same link and assert it runs the *lean* pair against the pinned
        // loopback PC and persists the new (endpoint, token, pin) — no model re-download, so the
        // only request the stub sees is the pair POST.
        val baseUrl = "https://127.0.0.1:$port"
        val uri = "idiolect://pair?u=${URLEncoder.encode(baseUrl, "UTF-8")}&c=$CODE&f=$PIN"

        targetContext.startActivity(
            Intent(targetContext, SettingsActivity::class.java)
                .setData(Uri.parse(uri))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
        )

        val config = SecureSyncConfig.keystoreBacked(targetContext.filesDir)
        var settings = config.load()
        var waited = 0L
        while (settings?.token != TOKEN && waited < TIMEOUT_MS) {
            Thread.sleep(POLL_MS)
            waited += POLL_MS
            settings = config.load()
        }

        assertNotNull(
            "the re-pair link never persisted via Settings; last request the PC stub saw: ${lastRequest.get()}",
            settings,
        )
        assertEquals("the settings re-pair POSTed to the pinned /v1/pair", "POST /v1/pair", lastRequest.get())
        assertEquals("Settings persisted the issued token", TOKEN, settings!!.token)
        assertEquals("Settings persisted the scanned pin", PIN, settings.pin)
        assertEquals("Settings persisted the endpoint", baseUrl, settings.baseUrl)
    }

    /** Read one HTTP/1.1 request; 201 + token for `POST /v1/pair`, 404 otherwise. */
    private fun handle(socket: Socket) {
        socket.use { conn ->
            val input = conn.getInputStream()
            val head = ByteArrayOutputStream()
            val one = ByteArray(1)
            while (true) {
                if (input.read(one) < 0) return
                head.write(one, 0, 1)
                val a = head.toByteArray()
                if (a.size >= 4 &&
                    a[a.size - 4] == '\r'.code.toByte() && a[a.size - 3] == '\n'.code.toByte() &&
                    a[a.size - 2] == '\r'.code.toByte() && a[a.size - 1] == '\n'.code.toByte()
                ) {
                    break
                }
            }
            val lines = head.toByteArray().toString(Charsets.US_ASCII).split("\r\n")
            val requestLine = lines.first().split(" ")
            val method = requestLine.getOrNull(0) ?: ""
            val path = requestLine.getOrNull(1) ?: ""
            lastRequest.set("$method $path")
            var contentLength = 0
            for (line in lines.drop(1)) {
                val colon = line.indexOf(':')
                if (colon <= 0) continue
                if (line.substring(0, colon).trim().lowercase() == "content-length") {
                    contentLength = line.substring(colon + 1).trim().toIntOrNull() ?: 0
                }
            }
            // Drain the body so the client's write completes cleanly.
            val body = ByteArray(contentLength)
            var read = 0
            while (read < contentLength) {
                val n = input.read(body, read, contentLength - read)
                if (n < 0) break
                read += n
            }
            val out = conn.getOutputStream()
            if (method == "POST" && path == "/v1/pair") {
                val json = """{"token":"$TOKEN","device_id":"emulator","user_id":"default"}"""
                write(out, 201, "Created", json.toByteArray())
            } else {
                // The post-pair model pull lands here; 404 fails it fast (config already saved).
                write(out, 404, "Not Found", ByteArray(0))
            }
        }
    }

    private fun write(out: OutputStream, code: Int, reason: String, body: ByteArray) {
        val head = buildString {
            append("HTTP/1.1 $code $reason\r\n")
            append("Content-Length: ${body.size}\r\n")
            append("Connection: close\r\n")
            append("\r\n")
        }
        out.write(head.toByteArray(Charsets.US_ASCII))
        out.write(body)
        out.flush()
    }

    companion object {
        private const val CODE = "ABCD1234"
        private const val TOKEN = "tok-deeplink-e2e"

        /** sha256 of the fixture cert's DER SubjectPublicKeyInfo (assets/pinning/server.p12). */
        private const val PIN = "35e1083863409f0a52912f562f973a0c56eb050d638672b25474069dd4ae6e00"
        private val PASSWORD = "changeit".toCharArray()
        private const val POLL_MS = 250L
        private const val TIMEOUT_MS = 20_000L
        private const val NEGATIVE_WAIT_MS = 5_000L
    }
}
