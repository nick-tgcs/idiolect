package org.idiolect.android.sync

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.io.OutputStream
import java.net.Socket
import java.security.KeyStore
import java.util.concurrent.atomic.AtomicReference
import javax.net.ssl.KeyManagerFactory
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLServerSocket
import kotlin.concurrent.thread

/**
 * The real [HttpPairingTransport] over a genuine TLS socket presenting the self-signed fixture
 * cert (`src/test/resources/pinning/server.p12`) — the host-runnable mirror of the Rust
 * `pairing_over_https` integration test and the on-device emulator e2e. Proves the default
 * secure path end to end on the JVM: a correct pin completes the pinned handshake (no CA, no
 * hostname match — the server cert's CN is `idiolect-sync`, the client connects to `127.0.0.1`)
 * and exchanges the code for a token; a wrong pin aborts the handshake before any request bytes
 * leave the client, and an `https` endpoint with no pin is refused outright.
 */
class HttpPairingTransportTlsTest {
    private lateinit var server: SSLServerSocket
    private lateinit var baseUrl: String
    private val received = AtomicReference<String>()
    private val serverError = AtomicReference<Throwable?>()

    @Before
    fun start() {
        val keyStore = KeyStore.getInstance("PKCS12")
        javaClass.getResourceAsStream("/pinning/server.p12")!!.use { keyStore.load(it, PASSWORD) }
        val kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm())
        kmf.init(keyStore, PASSWORD)
        val context = SSLContext.getInstance("TLS")
        context.init(kmf.keyManagers, null, null)
        server = context.serverSocketFactory.createServerSocket(0) as SSLServerSocket
        baseUrl = "https://127.0.0.1:${server.localPort}"
        thread(isDaemon = true) {
            while (!server.isClosed) {
                val socket = try {
                    server.accept()
                } catch (_: Exception) {
                    break
                }
                try {
                    handle(socket)
                } catch (e: Exception) {
                    // test teardown, or a client that aborted the handshake on a pin mismatch
                    serverError.set(e)
                }
            }
        }
    }

    @After
    fun stop() {
        server.close()
    }

    @Test
    fun a_matching_pin_completes_the_pinned_handshake_and_exchanges_the_code() {
        val response = try {
            HttpPairingTransport(baseUrl, pin = PIN).requestToken("GOODCODE", "pixel-7a")
        } catch (e: Exception) {
            throw AssertionError("client failed; server-side error: ${serverError.get()}", e)
        }
        assertEquals("tok-abc", response.token)
        assertEquals("pixel-7a", response.deviceId)
        assertTrue("the code crossed the TLS connection", received.get().contains(""""code":"GOODCODE""""))
    }

    @Test
    fun a_wrong_pin_aborts_the_handshake_before_any_request() {
        assertThrows(IOException::class.java) {
            HttpPairingTransport(baseUrl, pin = "0".repeat(64)).requestToken("GOODCODE", "pixel-7a")
        }
    }

    @Test
    fun an_https_endpoint_without_a_pin_is_refused() {
        // applyPinning refuses unpinned https rather than leaning on system trust (which would
        // reject the self-signed server anyway) — a clearer, earlier failure.
        assertThrows(IllegalArgumentException::class.java) {
            HttpPairingTransport(baseUrl, pin = null).requestToken("GOODCODE", "pixel-7a")
        }
    }

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
            var contentLength = 0
            for (line in lines.drop(1)) {
                val colon = line.indexOf(':')
                if (colon <= 0) continue
                if (line.substring(0, colon).trim().lowercase() == "content-length") {
                    contentLength = line.substring(colon + 1).trim().toIntOrNull() ?: 0
                }
            }
            val body = ByteArray(contentLength)
            var read = 0
            while (read < contentLength) {
                val n = input.read(body, read, contentLength - read)
                if (n < 0) break
                read += n
            }
            respond(conn.getOutputStream(), body.toString(Charsets.UTF_8))
        }
    }

    private fun respond(out: OutputStream, body: String) {
        received.set(body)
        if (body.contains(""""code":"GOODCODE"""")) {
            val json = """{"token":"tok-abc","device_id":"pixel-7a","user_id":"default"}"""
            write(out, 201, "Created", json.toByteArray())
        } else {
            write(out, 401, "Unauthorized", ByteArray(0))
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
        private val PASSWORD = "changeit".toCharArray()

        /** sha256 of the fixture cert's DER SubjectPublicKeyInfo (src/test/resources/pinning). */
        private const val PIN = "35e1083863409f0a52912f562f973a0c56eb050d638672b25474069dd4ae6e00"
    }
}
