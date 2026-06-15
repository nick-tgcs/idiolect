package org.idiolect.android.sync

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.OutputStream
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread

/**
 * Exercises the real [HttpPairingTransport] (HttpURLConnection POST) against a tiny
 * in-process HTTP/1.1 server on a raw `ServerSocket` — the same host-runnable seam
 * [HttpSyncTransportTest] uses (no emulator). Covers the actual network behaviour: the
 * JSON body carries the code + device_id, a `201` yields the parsed token, and a `401`
 * (bad/expired code) surfaces as an error so nothing is persisted.
 */
class HttpPairingTransportTest {
    private lateinit var server: ServerSocket
    private lateinit var baseUrl: String
    private val received = AtomicReference<String>()
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
            // Read the header block (until CRLFCRLF) before the body.
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
            var contentLength = 0
            for (line in lines.drop(1)) {
                val colon = line.indexOf(':')
                if (colon <= 0) continue
                val key = line.substring(0, colon).trim().lowercase()
                val value = line.substring(colon + 1).trim()
                if (key == "content-length") contentLength = value.toIntOrNull() ?: 0
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

    @Test
    fun a_correct_code_is_exchanged_for_a_token() {
        val response = HttpPairingTransport(baseUrl).requestToken("GOODCODE", "pixel-7a")

        assertEquals("POST", receivedMethod.get())
        assertEquals("/v1/pair", receivedPath.get())
        assertTrue("the code is sent", received.get().contains(""""code":"GOODCODE""""))
        assertTrue("the device id is sent", received.get().contains(""""device_id":"pixel-7a""""))
        assertEquals("tok-abc", response.token)
        assertEquals("pixel-7a", response.deviceId)
        assertEquals("default", response.userId)
    }

    @Test
    fun a_rejected_code_surfaces_as_an_error() {
        assertThrows(IllegalArgumentException::class.java) {
            HttpPairingTransport(baseUrl).requestToken("WRONGCOD", "pixel-7a")
        }
    }
}
