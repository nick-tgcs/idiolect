package org.idiolect.android.sync

/**
 * Orchestrates the phone-side S3 pairing: exchange a one-time code for a per-device
 * token and persist the endpoint so [OutboxPump] / [SyncWorker] can sync. Pure
 * coordination behind a [PairingTransport] seam (host-tested with a fake), exactly as
 * [OutboxPump] sits behind [SyncTransport] — the Activity that calls it stays dumb.
 *
 * The transport is built per call from the entered `baseUrl` via [transportFactory]
 * (defaulting to [HttpPairingTransport]); tests inject a fake.
 */
class PairingClient(
    private val config: SyncConfig,
    private val deviceId: String,
    private val transportFactory: (String) -> PairingTransport = ::HttpPairingTransport,
) {
    /**
     * Pair with the PC at [baseUrl] using the operator's one-time [code]: request a token
     * and, only on success, persist `(baseUrl, token)` so future syncs authenticate.
     * Returns the paired identity; throws (persisting nothing) if the code is rejected or
     * the server is unreachable.
     */
    fun pair(baseUrl: String, code: String): PairingResponse {
        val response = transportFactory(baseUrl).requestToken(code, deviceId)
        config.save(SyncSettings(baseUrl, response.token))
        return response
    }
}
