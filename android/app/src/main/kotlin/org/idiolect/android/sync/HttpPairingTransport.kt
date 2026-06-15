package org.idiolect.android.sync

import java.net.HttpURLConnection
import java.net.URL

/**
 * [PairingTransport] over HTTP to the user's PC (`idiolect-sync-server`'s `POST
 * /v1/pair`, S3). Uses `HttpURLConnection` (no third-party dependency — keeps the APK
 * lean and FOSS for GrapheneOS), mirroring [HttpSyncTransport]. The route is *not*
 * bearer-authenticated — the one-time code is the credential — and a successful pair
 * returns `201 Created` with the per-device token.
 */
class HttpPairingTransport(
    private val baseUrl: String,
    private val connectTimeoutMs: Int = 15_000,
    private val readTimeoutMs: Int = 30_000,
) : PairingTransport {
    override fun requestToken(code: String, deviceId: String): PairingResponse {
        val connection = URL(baseUrl.trimEnd('/') + "/v1/pair").openConnection() as HttpURLConnection
        connection.connectTimeout = connectTimeoutMs
        connection.readTimeout = readTimeoutMs
        connection.requestMethod = "POST"
        connection.doOutput = true
        connection.setRequestProperty("Content-Type", "application/json")
        val body = requestBody(code, deviceId).toByteArray()
        connection.setFixedLengthStreamingMode(body.size)
        try {
            connection.outputStream.use { it.write(body) }
            val status = connection.responseCode
            require(status == HttpURLConnection.HTTP_CREATED) { "pairing failed: HTTP $status" }
            val json = connection.inputStream.use { it.readBytes() }.toString(Charsets.UTF_8)
            return PairingResponse.parse(json)
        } finally {
            connection.disconnect()
        }
    }

    /** The pairing request JSON. Built by hand to stay dependency-free; both values are
     *  escaped so an oddly-typed code still produces a well-formed body. */
    private fun requestBody(code: String, deviceId: String): String =
        """{"code":"${escape(code)}","device_id":"${escape(deviceId)}"}"""

    private fun escape(value: String): String =
        value.replace("\\", "\\\\").replace("\"", "\\\"")
}
