package org.idiolect.android.model

import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Before
import org.junit.Test
import java.io.OutputStream
import java.io.ByteArrayOutputStream
import java.net.ServerSocket
import java.net.Socket
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * Exercises the real [HttpModelTransport] (HttpURLConnection) against a tiny in-process
 * HTTP/1.1 server built on a raw `ServerSocket` — `com.sun.net.httpserver` is not on the
 * Android unit-test classpath, but `java.net` is, so this stays host-runnable (no
 * emulator) while covering the actual network seam: bearer auth, JSON manifest, and a
 * Range-aware model endpoint.
 */
class HttpModelTransportTest {
    private lateinit var server: ServerSocket
    private lateinit var baseUrl: String
    private val running = AtomicBoolean(true)
    private val token = "secret-token"
    private val modelBytes = ByteArray(2500) { (it % 256).toByte() }
    private val digest =
        MessageDigest.getInstance("SHA-256").digest(modelBytes).joinToString("") { "%02x".format(it.toInt() and 0xFF) }

    @Before
    fun start() {
        server = ServerSocket(0)
        baseUrl = "http://127.0.0.1:${server.localPort}"
        thread(isDaemon = true) {
            while (running.get()) {
                val socket = try { server.accept() } catch (_: Exception) { break }
                try { handle(socket) } catch (_: Exception) { /* test teardown */ }
            }
        }
    }

    @After
    fun stop() {
        running.set(false)
        server.close()
    }

    private fun handle(socket: Socket) {
        socket.use { connection ->
            val reader = connection.getInputStream().bufferedReader()
            val requestLine = reader.readLine() ?: return
            val path = requestLine.split(" ").getOrNull(1) ?: ""
            var auth: String? = null
            var range: String? = null
            while (true) {
                val line = reader.readLine()
                if (line.isNullOrEmpty()) break
                val colon = line.indexOf(':')
                if (colon <= 0) continue
                val key = line.substring(0, colon).trim().lowercase()
                val value = line.substring(colon + 1).trim()
                when (key) {
                    "authorization" -> auth = value
                    "range" -> range = value
                }
            }
            respond(connection.getOutputStream(), path, auth, range)
        }
    }

    private fun respond(out: OutputStream, path: String, auth: String?, range: String?) {
        if (auth != "Bearer $token") {
            write(out, 401, "Unauthorized", emptyMap(), ByteArray(0))
            return
        }
        when (path) {
            "/v1/model/manifest" -> {
                val body = """{"id":"base.en","sha256":"$digest","size":${modelBytes.size}}""".toByteArray()
                write(out, 200, "OK", mapOf("Content-Type" to "application/json"), body)
            }
            "/v1/model" -> if (range != null) {
                val start = range.removePrefix("bytes=").substringBefore("-").toInt()
                val slice = modelBytes.copyOfRange(start, modelBytes.size)
                val header = mapOf("Content-Range" to "bytes $start-${modelBytes.size - 1}/${modelBytes.size}")
                write(out, 206, "Partial Content", header, slice)
            } else {
                write(out, 200, "OK", emptyMap(), modelBytes)
            }
            else -> write(out, 404, "Not Found", emptyMap(), ByteArray(0))
        }
    }

    private fun write(out: OutputStream, code: Int, reason: String, headers: Map<String, String>, body: ByteArray) {
        val head = buildString {
            append("HTTP/1.1 $code $reason\r\n")
            append("Content-Length: ${body.size}\r\n")
            append("Connection: close\r\n")
            headers.forEach { (key, value) -> append("$key: $value\r\n") }
            append("\r\n")
        }
        out.write(head.toByteArray(Charsets.US_ASCII))
        out.write(body)
        out.flush()
    }

    @Test
    fun fetch_manifest_parses_the_server_response() {
        val manifest = HttpModelTransport(baseUrl, token).fetchManifest()
        assertEquals(ModelManifest("base.en", digest, modelBytes.size.toLong()), manifest)
    }

    @Test
    fun a_full_download_streams_every_byte() {
        val sink = ByteArrayOutputStream()
        HttpModelTransport(baseUrl, token).download(0, sink) {}
        assertArrayEquals(modelBytes, sink.toByteArray())
    }

    @Test
    fun a_range_download_resumes_from_the_offset() {
        val sink = ByteArrayOutputStream()
        HttpModelTransport(baseUrl, token).download(1000, sink) {}
        assertArrayEquals(modelBytes.copyOfRange(1000, modelBytes.size), sink.toByteArray())
    }

    @Test
    fun a_wrong_token_is_rejected() {
        assertThrows(IllegalArgumentException::class.java) {
            HttpModelTransport(baseUrl, "wrong-token").fetchManifest()
        }
    }
}
