package org.idiolect.android.sync

/**
 * Orchestrates the phone-side S3 pairing: exchange a one-time code for a per-device
 * token and persist the endpoint so [OutboxPump] / [SyncWorker] can sync. Pure
 * coordination behind a [PairingTransport] seam (host-tested with a fake), exactly as
 * [OutboxPump] sits behind [SyncTransport] — the Activity that calls it stays dumb.
 *
 * The transport is built per call from the entered `baseUrl` and the scanned [pin] via
 * [transportFactory] (defaulting to [HttpPairingTransport]); tests inject a fake.
 */
class PairingClient(
    private val config: SecureSyncConfig,
    private val deviceId: String,
    private val transportFactory: (String, String?) -> PairingTransport =
        { baseUrl, pin -> HttpPairingTransport(baseUrl, pin) },
) {
    /**
     * Pair with the PC at [baseUrl] using the operator's one-time [code], pinning the
     * server cert to [pin] when the QR carried one (TLS, the default; null under `--no-tls`):
     * request a token and, only on success, persist `(baseUrl, token, pin)` so future syncs
     * authenticate *and* re-pin. Returns the paired identity; throws (persisting nothing) if
     * the code is rejected, the cert fails the pin, or the server is unreachable.
     */
    fun pair(baseUrl: String, code: String, pin: String?): PairingResponse {
        val response = transportFactory(baseUrl, pin).requestToken(code, deviceId)
        config.save(SyncSettings(baseUrl, response.token, pin))
        return response
    }
}
