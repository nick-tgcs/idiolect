package org.idiolect.android.model

import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import java.io.ByteArrayOutputStream
import java.io.OutputStream
import java.net.ServerSocket
import java.net.Socket
import java.nio.file.Files
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlin.concurrent.thread

/**
 * Exercises the real [PublicModelTransport] (HttpURLConnection) against a tiny in-process
 * HTTP/1.1 server on a raw loopback `ServerSocket` — the same host-runnable seam as
 * [HttpModelTransportTest], but for the PC-less mobile path: a fixed PUBLIC url, NO bearer
 * auth, and a digest PINNED in the app rather than served. The loopback exception to the
 * https requirement is what keeps this an emulator-free unit test.
 */
class PublicModelTransportTest {
    private lateinit var server: ServerSocket
    private lateinit var url: String
    private lateinit var redirectUrl: String
    private val running = AtomicBoolean(true)
    private val sawAuthHeader = AtomicBoolean(false)
    private val accepts = AtomicInteger(0)
    private val modelBytes = ByteArray(2500) { (it % 256).toByte() }

    // The real digest of the served bytes — the manifest the caller pins (the public CDN
    // serves only raw bytes, so unlike the PC path there is no manifest endpoint to fetch).
    private val realDigest =
        MessageDigest.getInstance("SHA-256").digest(modelBytes).joinToString("") { "%02x".format(it.toInt() and 0xFF) }
    private val pinned = ModelManifest("ggml-base.en", realDigest, modelBytes.size.toLong())

    @Before
    fun start() {
        server = ServerSocket(0)
        url = "http://127.0.0.1:${server.localPort}/ggml-base.en.bin"
        redirectUrl = "http://127.0.0.1:${server.localPort}/redirect"
        thread(isDaemon = true) {
            while (running.get()) {
                val socket = try { server.accept() } catch (_: Exception) { break }
                accepts.incrementAndGet()
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
            var range: String? = null
            while (true) {
                val line = reader.readLine()
                if (line.isNullOrEmpty()) break
                val colon = line.indexOf(':')
                if (colon <= 0) continue
                val key = line.substring(0, colon).trim().lowercase()
                val value = line.substring(colon + 1).trim()
                when (key) {
                    "authorization" -> sawAuthHeader.set(true)
                    "range" -> range = value
                }
            }
            respond(connection.getOutputStream(), path, range)
        }
    }

    private fun respond(out: OutputStream, path: String, range: String?) {
        if (path == "/redirect") {
            // A cross-protocol redirect (http origin → https target). HttpURLConnection
            // refuses to auto-follow it, so download() sees the 302 and rejects it — the
            // same refusal that stops an https→http downgrade against the real CDN.
            write(out, 302, "Found", mapOf("Location" to "https://127.0.0.1:1/blocked"), ByteArray(0))
            return
        }
        if (range != null) {
            val start = range.removePrefix("bytes=").substringBefore("-").toInt()
            write(
                out, 206, "Partial Content",
                mapOf("Content-Range" to "bytes $start-${modelBytes.size - 1}/${modelBytes.size}"),
                modelBytes.copyOfRange(start, modelBytes.size),
            )
        } else {
            write(out, 200, "OK", emptyMap(), modelBytes)
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
    fun fetch_manifest_returns_the_pinned_manifest_without_a_network_call() {
        // Pointed at the LIVE server: the manifest is pinned in the app, so fetching it must
        // not open a connection — proven directly by the server recording zero accepts.
        val transport = PublicModelTransport(url, pinned)
        assertEquals(pinned, transport.fetchManifest())
        assertEquals("fetchManifest must not touch the network", 0, accepts.get())
    }

    @Test
    fun a_full_download_streams_every_byte_with_no_auth_header() {
        val sink = ByteArrayOutputStream()
        PublicModelTransport(url, pinned).download(0, sink) {}
        assertArrayEquals(modelBytes, sink.toByteArray())
        assertFalse("the public CDN path must not send a bearer token", sawAuthHeader.get())
    }

    @Test
    fun a_range_download_resumes_from_the_offset() {
        val sink = ByteArrayOutputStream()
        PublicModelTransport(url, pinned).download(1000, sink) {}
        assertArrayEquals(modelBytes.copyOfRange(1000, modelBytes.size), sink.toByteArray())
    }

    @Test
    fun a_non_loopback_plaintext_url_is_rejected() {
        // Defense in depth: a model pulled over cleartext from a real host could be swapped
        // before the digest check, so only https (or loopback, for tests) is permitted.
        assertThrows(IllegalArgumentException::class.java) {
            PublicModelTransport("http://example.com/ggml-base.en.bin", pinned)
        }
    }

    @Test
    fun the_recommended_source_is_pinned_and_served_over_https() {
        assertTrue("default source must be https", PublicModelTransport.DEFAULT_URL.startsWith("https://"))
        val manifest = PublicModelTransport.recommended().fetchManifest()
        assertEquals("ggml-base.en", manifest.id)
        assertEquals("a full SHA-256 hex digest is pinned", 64, manifest.sha256.length)
        assertTrue("a positive byte size is pinned", manifest.size > 0)
    }

    @Test
    fun an_uppercase_https_scheme_is_accepted() {
        // The scheme check is case-insensitive — a secure URL must not be rejected as cleartext.
        val transport = PublicModelTransport("HTTPS://huggingface.co/ggml-base.en.bin", pinned)
        assertEquals(pinned, transport.fetchManifest())
    }

    @Test
    fun a_cross_protocol_redirect_is_refused_rather_than_followed_to_cleartext() {
        // HttpURLConnection will not auto-follow the http→https hop, so download() sees the
        // 302 and rejects it instead of streaming from the redirect target.
        val sink = ByteArrayOutputStream()
        assertThrows(IllegalArgumentException::class.java) {
            PublicModelTransport(redirectUrl, pinned).download(0, sink) {}
        }
        assertEquals("nothing is streamed from an unfollowed redirect", 0, sink.size())
    }

    @Test
    fun download_reports_the_running_byte_count() {
        var lastFull = 0L
        PublicModelTransport(url, pinned).download(0, ByteArrayOutputStream()) { lastFull = it }
        assertEquals(modelBytes.size.toLong(), lastFull)

        var lastRange = 0L
        PublicModelTransport(url, pinned).download(1000, ByteArrayOutputStream()) { lastRange = it }
        assertEquals("a resume reports the bytes streamed in this call", (modelBytes.size - 1000).toLong(), lastRange)
    }

    @Test
    fun the_downloader_installs_when_the_served_bytes_match_the_pinned_digest() {
        // The end-to-end mobile path: real transport → ModelDownloader verify → atomic install.
        val store = ModelStore(Files.createTempDirectory("public-dl").toFile())
        val installed = ModelDownloader(PublicModelTransport(url, pinned), store).download()
        assertEquals("ggml-base.en", installed.id)
        assertArrayEquals(modelBytes, store.modelFile("ggml-base.en").readBytes())
        assertEquals(installed, store.active())
    }

    @Test
    fun the_downloader_rejects_served_bytes_that_do_not_match_the_pinned_digest() {
        // A CDN that serves the wrong/tampered bytes: the pin is the sole integrity gate,
        // so the download must fail and install nothing.
        val store = ModelStore(Files.createTempDirectory("public-dl").toFile())
        val wrongPin = ModelManifest("ggml-base.en", "00".repeat(32), modelBytes.size.toLong())
        assertThrows(ModelIntegrityException::class.java) {
            ModelDownloader(PublicModelTransport(url, wrongPin), store).download()
        }
        assertFalse("a digest mismatch installs nothing", store.isInstalled("ggml-base.en"))
    }
}
