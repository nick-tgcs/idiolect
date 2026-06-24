package org.idiolect.android.sync

/**
 * Ships an encoded sync batch to the PC's `POST /v1/sync` ingest endpoint (M6). A
 * thin framework seam: the orchestration it serves ([OutboxPump]) is host-tested
 * with a fake, and the HTTP impl ([HttpSyncTransport]) is covered against an
 * in-JVM server.
 */
interface SyncTransport {
    /**
     * POST the [batch] (the bytes [SyncSource.exportBatch] produced). Returns
     * normally only when the PC acked the whole batch as durably stored (HTTP 200);
     * throws on any non-200 or transport failure, so the caller leaves the outbox
     * intact for a later retry.
     */
    fun postBatch(batch: ByteArray)
}
