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
        var postCount: Int = 0

        override fun postBatch(batch: ByteArray) {
            posted = batch
            postCount++
            if (fail) throw IOException("network down")
        }
    }

    /** Drains a queue of pending batches: each export pops one, an empty queue exports `[]`. */
    private class QueueSource(batches: List<ByteArray>) : SyncSource {
        private val pending = ArrayDeque(batches)
        val confirmed = mutableListOf<ByteArray>()

        override fun exportBatch(deviceId: String, batchId: String): ByteArray =
            pending.firstOrNull() ?: ByteArray(0)

        override fun confirmSynced(batch: ByteArray) {
            confirmed += batch
            pending.removeFirstOrNull()
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

    @Test
    fun drain_ships_every_pending_batch_until_the_outbox_is_empty() {
        val batches = listOf(byteArrayOf(1), byteArrayOf(2), byteArrayOf(3))
        val source = QueueSource(batches)
        val transport = FakeTransport()

        val shipped = OutboxPump(source, transport, "pixel") { "b" }.drain()

        assertEquals("every pending batch is shipped", 3, shipped)
        assertEquals(3, transport.postCount)
        assertEquals(3, source.confirmed.size)
    }

    @Test
    fun drain_on_an_empty_outbox_ships_nothing_and_touches_no_network() {
        val source = QueueSource(emptyList())
        val transport = FakeTransport()

        assertEquals(0, OutboxPump(source, transport, "pixel") { "b" }.drain())
        assertEquals("an empty outbox never hits the network", 0, transport.postCount)
    }

    @Test
    fun drain_stops_at_the_cap_so_a_stuck_outbox_cannot_loop_forever() {
        // A source that never empties (always returns a batch, never clears) — the cap
        // is the backstop against an infinite drain.
        val source = FakeSource(byteArrayOf(7))
        val transport = FakeTransport()

        val shipped = OutboxPump(source, transport, "pixel") { "b" }.drain(maxBatches = 5)

        assertEquals("the drain is bounded by the cap", 5, shipped)
        assertEquals(5, transport.postCount)
    }

    @Test
    fun drain_propagates_a_transport_failure_leaving_the_rest_pending() {
        val source = QueueSource(listOf(byteArrayOf(1), byteArrayOf(2)))
        val transport = FakeTransport(fail = true)

        assertThrows(IOException::class.java) {
            OutboxPump(source, transport, "pixel") { "b" }.drain()
        }
        // First batch posted (and threw) before any confirm — nothing reclaimed.
        assertEquals(0, source.confirmed.size)
    }
}
