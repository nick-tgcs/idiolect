package org.idiolect.android.sync

import java.io.File

/** Where the sync worker ships to: the PC's base URL and the bearer token. */
data class SyncSettings(val baseUrl: String, val token: String)

/**
 * The persisted sync endpoint, stored under the app's private `filesDir`. Written once at
 * setup (the same URL + token the user enters to download the model), read later by the
 * background [SyncWorker] in its own process restart.
 *
 * Format is two lines (`url`\n`token`) — same shape as [org.idiolect.android.model.ModelStore]'s
 * active record. The token is plaintext on disk for M6; S3's pairing flow replaces this with
 * a per-device token wrapped by the AndroidKeyStore.
 */
class SyncConfig(private val file: File) {
    fun save(settings: SyncSettings) {
        file.parentFile?.mkdirs()
        file.writeText("${settings.baseUrl}\n${settings.token}")
    }

    /** The saved endpoint, or `null` if nothing is configured yet (or the file is truncated). */
    fun load(): SyncSettings? {
        val lines = file.takeIf { it.exists() }?.readLines() ?: return null
        if (lines.size < 2) return null
        return SyncSettings(lines[0], lines[1])
    }

    companion object {
        const val FILE_NAME = "sync.config"
    }
}
