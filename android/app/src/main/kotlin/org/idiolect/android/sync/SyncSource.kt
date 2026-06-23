package org.idiolect.android.sync

/**
 * The local outbox as the pump sees it — the phone half of learning-sync, backed by
 * the native core ([CoreSyncSource]). Kept behind an interface so [OutboxPump] is
 * host-testable with a fake (the real core loads the `.so`, which the unit tests
 * cannot).
 */
interface SyncSource {
    /**
     * Encode the pending (captured, not-yet-shipped) raw→corrected learnings + their
     * audio into the on-the-wire sync container. Returns an **empty** array when
     * nothing is pending, so the pump can skip the network entirely.
     */
    fun exportBatch(deviceId: String, batchId: String): ByteArray

    /**
     * After the PC acked [batch] as durably stored, reclaim local storage for it
     * (flip the learnings to synced and drop their on-device audio).
     */
    fun confirmSynced(batch: ByteArray)
}
