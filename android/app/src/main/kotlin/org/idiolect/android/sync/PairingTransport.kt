package org.idiolect.android.sync

/**
 * Exchanges a one-time pairing code for a per-device bearer token at the PC's
 * `POST /v1/pair` (S3). A thin framework seam: the orchestration it serves
 * ([PairingClient]) is host-tested with a fake, and the HTTP impl
 * ([HttpPairingTransport]) is covered against an in-JVM server.
 */
interface PairingTransport {
    /**
     * POST the one-time [code] and the device's [deviceId] and return the issued token
     * on success (HTTP 201). Throws on a rejected code, a malformed response, or any
     * transport failure, so the caller can surface "pairing failed, check the code"
     * without persisting anything.
     */
    fun requestToken(code: String, deviceId: String): PairingResponse
}
