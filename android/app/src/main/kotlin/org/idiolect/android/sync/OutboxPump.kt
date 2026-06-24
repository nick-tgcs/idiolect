package org.idiolect.android.sync

import java.util.UUID

/**
 * Drains the local sync outbox to the PC once: export the pending learnings, POST
 * them, and — only after the PC acks — confirm (reclaim) them. Pure logic over
 * [SyncSource] + [SyncTransport], so it is host-tested with fakes; the WorkManager
 * worker is the thin scheduling glue around it.
 *
 * `delete-after-ACK` ordering is the safety invariant: [SyncSource.confirmSynced]
 * runs **only** after [SyncTransport.postBatch] returns, so a failed upload leaves
 * the outbox intact for the next run. `batchId` is minted fresh per attempt; the
 * server dedups by content digest, so re-shipping after a failure is safe.
 */
class OutboxPump(
    private val source: SyncSource,
    private val transport: SyncTransport,
    private val deviceId: String,
    private val batchId: () -> String = { UUID.randomUUID().toString() },
) {
    /**
     * Ship the currently-pending batch. Returns `true` if a batch was shipped and
     * reclaimed, `false` if the outbox was empty. Propagates a transport failure
     * (nothing is reclaimed) so the caller can retry.
     */
    fun pumpOnce(): Boolean {
        val batch = source.exportBatch(deviceId, batchId())
        if (batch.isEmpty()) return false
        transport.postBatch(batch)
        source.confirmSynced(batch)
        return true
    }

    /**
     * Ship every pending batch, stopping when the outbox empties. Returns how many batches
     * were shipped. A transport failure propagates (the remaining batches stay pending for
     * the next run). [maxBatches] bounds the loop so a pathological outbox that never clears
     * cannot spin forever — the worker just retries on its next scheduled run.
     */
    fun drain(maxBatches: Int = 64): Int {
        var shipped = 0
        while (shipped < maxBatches && pumpOnce()) shipped++
        return shipped
    }
}
