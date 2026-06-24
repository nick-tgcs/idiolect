package org.idiolect.android.sync

import java.io.File
import java.util.UUID

/**
 * The stable per-install device identifier. Minted once (a random UUID) and persisted
 * under `filesDir`, so it is the same for the IME process and the background [SyncWorker]
 * process, and survives restarts. The PC pairs ingest idempotency on `(device_id,
 * audio_digest)`, so this value must not change between shipments of the same learning.
 */
class DeviceId(private val file: File) {
    fun get(): String {
        file.takeIf { it.exists() }?.readText()?.trim()?.takeIf { it.isNotEmpty() }?.let { return it }
        val id = UUID.randomUUID().toString()
        file.parentFile?.mkdirs()
        file.writeText(id)
        return id
    }

    companion object {
        const val FILE_NAME = "device.id"
    }
}
