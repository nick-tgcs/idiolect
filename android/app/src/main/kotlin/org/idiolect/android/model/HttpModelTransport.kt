package org.idiolect.android.model

import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URL

/**
 * [ModelTransport] over HTTP to the user's PC (`idiolect-sync-server`, M5b). Uses
 * `HttpURLConnection` (no third-party dependency — keeps the APK lean and FOSS for
 * GrapheneOS), sends the bearer token, and asks for a byte Range to resume. This is a
 * thin framework seam; the orchestration it serves is host-tested via a fake, and the
 * seam itself is covered by a host test against an in-JVM HTTP server.
 */
class HttpModelTransport(
    private val baseUrl: String,
    private val token: String,
    private val connectTimeoutMs: Int = 15_000,
    private val readTimeoutMs: Int = 60_000,
) : ModelTransport {
    override fun fetchManifest(): ModelManifest {
        val connection = open("/v1/model/manifest")
        try {
            require(connection.responseCode == HttpURLConnection.HTTP_OK) {
                "manifest request failed: HTTP ${connection.responseCode}"
            }
            val json = connection.inputStream.bufferedReader().use { it.readText() }
            return ModelManifest.parse(json)
        } finally {
            connection.disconnect()
        }
    }

    override fun download(offset: Long, sink: OutputStream, onBytes: (Long) -> Unit) {
        val connection = open("/v1/model")
        if (offset > 0) connection.setRequestProperty("Range", "bytes=$offset-")
        try {
            val code = connection.responseCode
            require(code == HttpURLConnection.HTTP_OK || code == HttpURLConnection.HTTP_PARTIAL) {
                "model download failed: HTTP $code"
            }
            connection.inputStream.use { input ->
                val buffer = ByteArray(64 * 1024)
                var total = 0L
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    sink.write(buffer, 0, read)
                    total += read
                    onBytes(total)
                }
            }
        } finally {
            connection.disconnect()
        }
    }

    private fun open(path: String): HttpURLConnection {
        val connection = URL(baseUrl.trimEnd('/') + path).openConnection() as HttpURLConnection
        connection.connectTimeout = connectTimeoutMs
        connection.readTimeout = readTimeoutMs
        connection.setRequestProperty("Authorization", "Bearer $token")
        return connection
    }
}
