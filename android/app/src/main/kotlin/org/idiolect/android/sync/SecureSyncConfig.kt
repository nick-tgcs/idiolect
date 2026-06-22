package org.idiolect.android.sync

import java.io.File

/**
 * Where the sync worker ships to: the PC's base URL, the per-device bearer token, and — when
 * the endpoint is TLS (the default) — the server cert's SPKI [pin] the phone scanned on
 * pairing. A null [pin] is a cleartext (`--no-tls`) endpoint.
 */
data class SyncSettings(val baseUrl: String, val token: String, val pin: String? = null)

/**
 * The persisted sync endpoint, stored under the app's private `filesDir`. Written once at
 * pairing/setup, read later by the background [SyncWorker] in its own process restart.
 *
 * The non-secret base URL is stored in the clear ([URL_FILE_NAME]); the bearer token is
 * wrapped by the AndroidKeyStore via [PairingTokenStore], so a token never lands on disk in
 * plaintext. The [pin][SyncSettings.pin] is a public-key hash (not a secret), so it too lives
 * in the clear ([PIN_FILE_NAME]) — it must survive restarts so every later sync re-pins the
 * server. An endpoint only counts as configured when the URL **and** token are present — a
 * half-written or unpaired device [load]s as `null`; a missing pin is allowed (cleartext).
 */
class SecureSyncConfig(
    private val urlFile: File,
    private val tokenStore: PairingTokenStore,
    private val pinFile: File,
) {
    fun save(settings: SyncSettings) {
        urlFile.parentFile?.mkdirs()
        urlFile.writeText(settings.baseUrl)
        // The pin is non-secret and optional: write it when present, else clear any stale one
        // so re-pairing a cleartext endpoint can't keep pinning the old cert.
        if (settings.pin != null) {
            pinFile.writeText(settings.pin)
        } else {
            pinFile.delete()
        }
        // Write the token last so a crash mid-save never leaves a usable (url, token) pair.
        tokenStore.save(settings.token)
    }

    /**
     * Unpair: wipe the endpoint entirely — URL, pin, and the wrapped token — so the device
     * reads as unconfigured ([load] → `null`) and no stale credential or cert pin survives.
     * Driven by the settings screen's Unpair action; a no-op on an already-unpaired device.
     */
    fun clear() {
        urlFile.delete()
        pinFile.delete()
        tokenStore.clear()
    }

    /** The saved endpoint, or `null` if the URL or token is missing (unconfigured / unpaired). */
    fun load(): SyncSettings? {
        val baseUrl = urlFile.takeIf { it.exists() }?.readText()?.trim()?.takeIf { it.isNotEmpty() }
            ?: return null
        val token = tokenStore.load() ?: return null
        val pin = pinFile.takeIf { it.exists() }?.readText()?.trim()?.takeIf { it.isNotEmpty() }
        return SyncSettings(baseUrl, token, pin)
    }

    companion object {
        const val URL_FILE_NAME = "sync.url"
        const val PIN_FILE_NAME = "sync.pin"

        /** The on-device store: URL + pin in the clear, token wrapped by the AndroidKeyStore. */
        fun keystoreBacked(filesDir: File) = SecureSyncConfig(
            urlFile = File(filesDir, URL_FILE_NAME),
            tokenStore = PairingTokenStore.keystoreBacked(File(filesDir, PairingTokenStore.FILE_NAME)),
            pinFile = File(filesDir, PIN_FILE_NAME),
        )
    }
}
