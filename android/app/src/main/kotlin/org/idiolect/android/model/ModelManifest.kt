package org.idiolect.android.model

/**
 * The served model's identity + integrity metadata, mirroring the desktop
 * `idiolect-sync-server` `GET /v1/model/manifest` response. [sha256] is the canonical
 * lowercase-hex digest the device verifies the download against and re-checks at load.
 */
data class ModelManifest(val id: String, val sha256: String, val size: Long) {
    companion object {
        /**
         * Parse the manifest JSON. The shape is fixed and server-controlled
         * (`{"id":..,"sha256":..,"size":..}`), so a small, dependency-free extractor
         * keeps this host-testable without `org.json` (which only works under
         * Robolectric). Throws [IllegalArgumentException] if a field is missing.
         */
        fun parse(json: String): ModelManifest {
            val id = stringField(json, "id")
            val sha256 = stringField(json, "sha256")
            val size = longField(json, "size")
            require(id != null && sha256 != null && size != null) {
                "malformed model manifest: $json"
            }
            return ModelManifest(id, sha256, size)
        }

        private fun stringField(json: String, key: String): String? =
            Regex(""""$key"\s*:\s*"([^"]*)"""").find(json)?.groupValues?.get(1)

        private fun longField(json: String, key: String): Long? =
            Regex(""""$key"\s*:\s*(\d+)""").find(json)?.groupValues?.get(1)?.toLong()
    }
}

/** An installed, integrity-verified model on the device. */
data class InstalledModel(val id: String, val sha256: String, val path: String)

/** A model download/install failed its SHA-256 integrity check. */
class ModelIntegrityException(expected: String, actual: String) :
    Exception("model integrity check failed: expected $expected, got $actual")

/** A model download was cancelled (or superseded) after verifying but before install, so
 *  nothing was installed. */
class ModelDownloadCancelledException :
    Exception("model download cancelled before install")
