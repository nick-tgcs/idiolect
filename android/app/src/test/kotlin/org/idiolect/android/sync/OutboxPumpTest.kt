package org.idiolect.android.sync

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.IOException

/**
 * The pump's logic over [SyncSource] + [SyncTransport], host-tested with fakes (the
 * real [SyncSource] is the native core, exercised on device). Mirrors how
 * `ModelDownloaderTest` fakes the transport.
 */
class OutboxPumpTest {
    private class FakeSource(private val batch: ByteArray) : SyncSource {
        var exportedDevice: String? = null
        var exportedBatchId: String? = null
        var confirmed: ByteArray? = null

        override fun exportBatch(deviceId: String, batchId: String): ByteArray {
            exportedDevice = deviceId
            exportedBatchId = batchId
            return batch
        }

        override fun confirmSynced(batch: ByteArray) {
            confirmed = batch
        }
    }

    private class FakeTransport(private val fail: Boolean = false) : SyncTransport {
        var posted: ByteArray? = null

        override fun postBatch(batch: ByteArray) {
            posted = batch
            if (fail) throw IOException("network down")
        }
    }

    @Test
    fun an_empty_outbox_ships_nothing() {
        val source = FakeSource(ByteArray(0))
        val transport = FakeTransport()

        val shipped = OutboxPump(source, transport, "pixel") { "batch-1" }.pumpOnce()

        assertFalse("an empty outbox is a no-op", shipped)
        assertNull("nothing is posted", transport.posted)
        assertNull("nothing is confirmed", source.confirmed)
    }

    @Test
    fun a_pending_batch_is_posted_then_confirmed() {
        val batch = byteArrayOf(1, 2, 3)
        val source = FakeSource(batch)
        val transport = FakeTransport()

        val shipped = OutboxPump(source, transport, "pixel") { "batch-9" }.pumpOnce()

        assertTrue("a non-empty batch ships", shipped)
        assertEquals("pixel", source.exportedDevice)
        assertEquals("batch-9", source.exportedBatchId)
        assertArrayEquals("the exported bytes are POSTed verbatim", batch, transport.posted)
        // Only after the POST succeeds is the batch confirmed (delete-after-ACK).
        assertArrayEquals("the shipped batch is confirmed", batch, source.confirmed)
    }

    @Test
    fun a_failed_post_does_not_confirm_so_the_outbox_survives_for_retry() {
        val batch = byteArrayOf(4, 5)
        val source = FakeSource(batch)
        val transport = FakeTransport(fail = true)

        assertThrows(IOException::class.java) {
            OutboxPump(source, transport, "pixel") { "b" }.pumpOnce()
        }
        // The PC never acked, so the candidates stay pending — the next run retries.
        assertNull("a failed ship reclaims nothing", source.confirmed)
    }
}
