package org.idiolect.android.sync

import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Before
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.OutputStream
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread

/**
 * Exercises the real [HttpSyncTransport] (HttpURLConnection POST) against a tiny
 * in-process HTTP/1.1 server on a raw `ServerSocket` — `com.sun.net.httpserver` is
 * not on the Android unit-test classpath, but `java.net` is, so this stays
 * host-runnable (no emulator) while covering the actual network seam: the POST body
 * is delivered verbatim, the bearer is sent, and a non-200 is surfaced as an error.
 */
class HttpSyncTransportTest {
    private lateinit var server: ServerSocket
    private lateinit var baseUrl: String
    private val token = "secret-token"
    private val received = AtomicReference<ByteArray>()
    private val receivedPath = AtomicReference<String>()
    private val receivedMethod = AtomicReference<String>()

    @Before
    fun start() {
        server = ServerSocket(0)
        baseUrl = "http://127.0.0.1:${server.localPort}"
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
                    // test teardown
                }
            }
        }
    }

    @After
    fun stop() {
        server.close()
    }

    private fun handle(socket: Socket) {
        socket.use { conn ->
            val input = conn.getInputStream()
            // Read the header block (bytes, until CRLFCRLF) so the binary body that
            // follows is not mangled by a char reader.
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
            receivedMethod.set(requestLine.getOrNull(0) ?: "")
            receivedPath.set(requestLine.getOrNull(1) ?: "")
            var auth: String? = null
            var contentLength = 0
            for (line in lines.drop(1)) {
                val colon = line.indexOf(':')
                if (colon <= 0) continue
                val key = line.substring(0, colon).trim().lowercase()
                val value = line.substring(colon + 1).trim()
                when (key) {
                    "authorization" -> auth = value
                    "content-length" -> contentLength = value.toIntOrNull() ?: 0
                }
            }
            val body = ByteArray(contentLength)
            var read = 0
            while (read < contentLength) {
                val n = input.read(body, read, contentLength - read)
                if (n < 0) break
                read += n
            }
            respond(conn.getOutputStream(), auth, body)
        }
    }

    private fun respond(out: OutputStream, auth: String?, body: ByteArray) {
        if (auth != "Bearer $token") {
            write(out, 401, "Unauthorized", ByteArray(0))
            return
        }
        received.set(body)
        val ack = """{"accepted":[],"already_have":[]}""".toByteArray()
        write(out, 200, "OK", ack)
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

    @Test
    fun a_batch_is_posted_to_the_sync_endpoint_with_the_bearer() {
        val batch = ByteArray(2500) { (it % 256).toByte() }
        HttpSyncTransport(baseUrl, token).postBatch(batch)

        assertEquals("POST", receivedMethod.get())
        assertEquals("/v1/sync", receivedPath.get())
        assertArrayEquals("the body is delivered verbatim, binary intact", batch, received.get())
    }

    @Test
    fun a_wrong_token_is_rejected() {
        assertThrows(IllegalArgumentException::class.java) {
            HttpSyncTransport(baseUrl, "wrong-token").postBatch(byteArrayOf(1, 2, 3))
        }
    }
}
