package org.idiolect.android.sync

/**
 * The outcome of pairing from a scan: the paired endpoint, its per-device token, and — under
 * TLS (the default) — the cert [pin] the QR carried, so the caller's follow-on model pull
 * reaches the same pinned cert.
 */
data class PairedEndpoint(val baseUrl: String, val token: String, val pin: String? = null)

/**
 * Turns a scanned pairing QR into a paired device: parse the [PairingUri], then exchange
 * the one-time code for a per-device token via [PairingClient] (which persists the endpoint
 * + token on success). Pure coordination behind the same seams [PairingClient] already
 * uses, so it is host-tested with a fake transport; the camera/Activity glue that produces
 * the scanned string has no headless seam and is covered by the manual emulator e2e.
 *
 * A malformed scan throws out of [PairingUri.parse] before any network call, so a stray QR
 * neither contacts a server nor persists anything.
 */
class ScanPairing(private val pairingClient: PairingClient) {
    fun pairFromScan(scanned: String): PairedEndpoint {
        val pairing = PairingUri.parse(scanned)
        val response = pairingClient.pair(pairing.baseUrl, pairing.code, pairing.pin)
        return PairedEndpoint(pairing.baseUrl, response.token, pairing.pin)
    }
}
