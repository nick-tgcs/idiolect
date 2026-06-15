package org.idiolect.android.sync

import java.io.File

/** Where the sync worker ships to: the PC's base URL and the per-device bearer token. */
data class SyncSettings(val baseUrl: String, val token: String)

/**
 * The persisted sync endpoint, stored under the app's private `filesDir`. Written once at
 * pairing/setup, read later by the background [SyncWorker] in its own process restart.
 *
 * The non-secret base URL is stored in the clear ([URL_FILE_NAME]); the bearer token is
 * wrapped by the AndroidKeyStore via [PairingTokenStore], so a token never lands on disk in
 * plaintext. An endpoint only counts as configured when *both* parts are present — a
 * half-written or unpaired device [load]s as `null`, so the worker treats it as "nothing to
 * sync to" rather than crashing.
 */
class SecureSyncConfig(
    private val urlFile: File,
    private val tokenStore: PairingTokenStore,
) {
    fun save(settings: SyncSettings) {
        urlFile.parentFile?.mkdirs()
        urlFile.writeText(settings.baseUrl)
        // Write the token last so a crash mid-save never leaves a usable (url, token) pair.
        tokenStore.save(settings.token)
    }

    /** The saved endpoint, or `null` if either part is missing (unconfigured / unpaired). */
    fun load(): SyncSettings? {
        val baseUrl = urlFile.takeIf { it.exists() }?.readText()?.trim()?.takeIf { it.isNotEmpty() }
            ?: return null
        val token = tokenStore.load() ?: return null
        return SyncSettings(baseUrl, token)
    }

    companion object {
        const val URL_FILE_NAME = "sync.url"

        /** The on-device store: URL in the clear, token wrapped by the AndroidKeyStore. */
        fun keystoreBacked(filesDir: File) = SecureSyncConfig(
            urlFile = File(filesDir, URL_FILE_NAME),
            tokenStore = PairingTokenStore.keystoreBacked(File(filesDir, PairingTokenStore.FILE_NAME)),
        )
    }
}
