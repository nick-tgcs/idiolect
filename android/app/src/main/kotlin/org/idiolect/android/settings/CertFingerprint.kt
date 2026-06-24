package org.idiolect.android.settings

/**
 * Display formatting for the pinned sync-server cert's SPKI fingerprint (the `f=` the pairing
 * QR carried, persisted by [org.idiolect.android.sync.SecureSyncConfig]). The paired-PC card
 * shows it so a human can verify it against what `idiolect-sync-server --pair` printed; a
 * 64-hex-char run is unreadable, so we break it into quads. Pure, total, never throws.
 */
object CertFingerprint {
    /** Group into 4-char chunks separated by spaces: `94e32367…` → `94e3 2367 …`. */
    fun grouped(fingerprint: String, group: Int = 4): String =
        fingerprint.chunked(group).joinToString(" ")

    /** The last quad with a leading ellipsis for a compact one-line reference: `…6c3c`. */
    fun short(fingerprint: String, tail: Int = 4): String =
        if (fingerprint.length <= tail) fingerprint else "…" + fingerprint.takeLast(tail)
}
