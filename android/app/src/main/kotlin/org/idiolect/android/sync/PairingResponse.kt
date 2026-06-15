package org.idiolect.android.sync

/**
 * The PC's response to a successful pairing (`idiolect-sync-server`'s `POST /v1/pair`,
 * S3): the per-device bearer [token] the device stores and then presents to `/v1/sync`
 * and `/v1/model`, plus the identity it was bound to.
 */
data class PairingResponse(val token: String, val deviceId: String, val userId: String) {
    companion object {
        /**
         * Parse the pairing JSON (`{"token":..,"device_id":..,"user_id":..}`). The shape
         * is fixed and server-controlled, so a small dependency-free extractor keeps this
         * host-testable without `org.json` (which only works under Robolectric), matching
         * `ModelManifest.parse`. Throws [IllegalArgumentException] if a field is missing.
         */
        fun parse(json: String): PairingResponse {
            val token = stringField(json, "token")
            val deviceId = stringField(json, "device_id")
            val userId = stringField(json, "user_id")
            require(token != null && deviceId != null && userId != null) {
                "malformed pairing response: $json"
            }
            return PairingResponse(token, deviceId, userId)
        }

        private fun stringField(json: String, key: String): String? =
            Regex(""""$key"\s*:\s*"([^"]*)"""").find(json)?.groupValues?.get(1)
    }
}
