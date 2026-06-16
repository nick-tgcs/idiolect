package org.idiolect.android.sync

import java.net.URLDecoder

/** What a scanned pairing QR carries: the PC's base URL and the one-time pairing code. */
data class ScannedPairing(val baseUrl: String, val code: String)

/**
 * Parses the pairing URI a PC's `--pair` QR encodes:
 * `idiolect://pair?u=<percent-encoded base URL>&c=<code>`. The exact inverse of the Rust
 * `pairing_uri` (in `idiolect-sync-server`'s `pairing_qr` module) — the two are a contract,
 * kept in lockstep by matching test literals on both sides.
 *
 * Dependency-free string parsing (no `android.net.Uri`) so it is host-testable without
 * Robolectric, matching [PairingResponse.parse]. The base URL is percent-decoded; the code
 * is from the pairing alphabet and carries no escapes. Any QR that is not a well-formed
 * pairing URI throws [IllegalArgumentException], so a stray scan can never be mistaken for
 * a pairing.
 */
object PairingUri {
    private const val PREFIX = "idiolect://pair?"

    fun parse(scanned: String): ScannedPairing {
        require(scanned.startsWith(PREFIX)) { "not a pairing QR: $scanned" }
        val params = HashMap<String, String>()
        for (field in scanned.removePrefix(PREFIX).split("&")) {
            val eq = field.indexOf('=')
            require(eq > 0) { "malformed pairing parameter: $field" }
            params[field.substring(0, eq)] = field.substring(eq + 1)
        }
        val baseUrl = params["u"]?.let { URLDecoder.decode(it, "UTF-8") }
        val code = params["c"]
        require(!baseUrl.isNullOrEmpty() && !code.isNullOrEmpty()) {
            "pairing QR is missing the url and/or code: $scanned"
        }
        return ScannedPairing(baseUrl, code)
    }
}
