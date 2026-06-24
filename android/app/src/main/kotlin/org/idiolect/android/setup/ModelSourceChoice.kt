package org.idiolect.android.setup

/**
 * Which model source the download form's two fields select. A pure decision so the routing
 * — integrity-pinned public model vs. an arbitrary user-supplied PC URL — is unit-tested
 * rather than buried in [SetupActivity]'s view glue (the [ImeSetup] pattern).
 */
sealed interface ModelSourceChoice {
    /** Both fields blank → the zero-config, integrity-pinned public model (PC-less path). */
    data object Public : ModelSourceChoice

    /**
     * Both fields filled → pull from the user's PC and remember the endpoint for sync. The
     * [pin] is the server cert's SPKI fingerprint carried by a pairing scan (TLS, the
     * default); manual URL+token entry leaves it null (cleartext, or an unpinned `https` the
     * transport will refuse).
     */
    data class Pc(val url: String, val token: String, val pin: String? = null) : ModelSourceChoice

    /** Exactly one field filled → a half-typed PC form; prompt for the missing one. */
    data object NeedDetails : ModelSourceChoice

    companion object {
        fun from(url: String, token: String): ModelSourceChoice {
            val trimmedUrl = url.trim()
            val trimmedToken = token.trim()
            return when {
                trimmedUrl.isEmpty() && trimmedToken.isEmpty() -> Public
                trimmedUrl.isNotEmpty() && trimmedToken.isNotEmpty() -> Pc(trimmedUrl, trimmedToken)
                else -> NeedDetails
            }
        }
    }
}
