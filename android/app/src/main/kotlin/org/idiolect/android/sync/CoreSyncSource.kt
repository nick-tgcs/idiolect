package org.idiolect.android.sync

import org.idiolect.ffi.IdiolectCore

/**
 * [SyncSource] backed by the native [IdiolectCore] — `export_sync_batch` /
 * `confirm_synced` over the FFI (M6). A one-line adapter; the pump logic it feeds is
 * host-tested, and the native round-trip is proven by the Rust seam test + the
 * on-device e2e.
 */
class CoreSyncSource(private val core: IdiolectCore) : SyncSource {
    override fun exportBatch(deviceId: String, batchId: String): ByteArray =
        core.exportSyncBatch(deviceId, batchId)

    override fun confirmSynced(batch: ByteArray) = core.confirmSynced(batch)
}
