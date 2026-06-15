package org.idiolect.android.sync

import java.net.HttpURLConnection
import java.net.URL

/**
 * [SyncTransport] over HTTP to the user's PC (`idiolect-sync-server`'s `POST
 * /v1/sync`, M6). Uses `HttpURLConnection` (no third-party dependency — keeps the
 * APK lean and FOSS for GrapheneOS) and sends the bearer token. The body is the
 * length-prefixed sync container (`application/vnd.idiolect.sync.v1`).
 */
class HttpSyncTransport(
    private val baseUrl: String,
    private val token: String,
    private val connectTimeoutMs: Int = 15_000,
    private val readTimeoutMs: Int = 60_000,
) : SyncTransport {
    override fun postBatch(batch: ByteArray) {
        val connection = URL(baseUrl.trimEnd('/') + "/v1/sync").openConnection() as HttpURLConnection
        connection.connectTimeout = connectTimeoutMs
        connection.readTimeout = readTimeoutMs
        connection.requestMethod = "POST"
        connection.doOutput = true
        connection.setRequestProperty("Authorization", "Bearer $token")
        connection.setRequestProperty("Content-Type", "application/vnd.idiolect.sync.v1")
        connection.setFixedLengthStreamingMode(batch.size)
        try {
            connection.outputStream.use { it.write(batch) }
            val code = connection.responseCode
            require(code == HttpURLConnection.HTTP_OK) { "sync upload failed: HTTP $code" }
            // Drain the ack body so the connection releases cleanly.
            connection.inputStream.use { it.readBytes() }
        } finally {
            connection.disconnect()
        }
    }
}
