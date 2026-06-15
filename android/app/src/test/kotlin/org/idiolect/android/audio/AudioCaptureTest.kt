package org.idiolect.android.audio

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit tests for the headless audio plumbing: the [PcmFrameQueue] relay that
 * decouples the capture thread from the (blocking) core push, and the [AudioCapture]
 * read loop over a [PcmSource]. Pure JVM — `AudioRecord` lives behind [PcmSource].
 */
class AudioCaptureTest {
    /** A [PcmSource] that yields a fixed script of reads, then reports stopped. */
    private class ScriptedSource(private val chunks: List<ShortArray>) : PcmSource {
        val events = mutableListOf<String>()
        private var index = 0

        override fun start() {
            events.add("start")
        }

        override fun read(into: ShortArray): Int {
            if (index >= chunks.size) return 0
            val chunk = chunks[index++]
            chunk.copyInto(into)
            return chunk.size
        }

        override fun stop() {
            events.add("stop")
        }
    }

    @Test
    fun the_queue_delivers_frames_in_order_then_stops_at_close() {
        val queue = PcmFrameQueue()
        queue.offer(shortArrayOf(1, 2))
        queue.offer(shortArrayOf(3))
        queue.close()

        val got = mutableListOf<List<Short>>()
        queue.consume { got.add(it.toList()) } // returns: the poison pill is queued

        assertEquals(listOf(listOf<Short>(1, 2), listOf<Short>(3)), got)
    }

    @Test
    fun the_queue_ignores_frames_offered_after_close() {
        val queue = PcmFrameQueue()
        queue.close()
        queue.offer(shortArrayOf(9))

        val got = mutableListOf<List<Short>>()
        queue.consume { got.add(it.toList()) }

        assertTrue("nothing delivered after close", got.isEmpty())
    }

    @Test
    fun capture_offers_each_read_then_closes_the_queue_on_stop() {
        val source = ScriptedSource(listOf(shortArrayOf(1, 2, 3), shortArrayOf(4, 5)))
        val queue = PcmFrameQueue()

        AudioCapture(source, queue, bufferSamples = 8).run()

        val got = mutableListOf<List<Short>>()
        queue.consume { got.add(it.toList()) } // run() already closed the queue

        assertEquals(listOf(listOf<Short>(1, 2, 3), listOf<Short>(4, 5)), got)
        // The source is started before the loop and stopped after it drains.
        assertEquals(listOf("start", "stop"), source.events)
    }

    @Test
    fun capture_copies_only_the_samples_actually_read() {
        // A short read (fewer samples than the buffer) must not leak stale tail bytes.
        val source = ScriptedSource(listOf(shortArrayOf(7, 7)))
        val queue = PcmFrameQueue()

        AudioCapture(source, queue, bufferSamples = 16).run()

        val got = mutableListOf<List<Short>>()
        queue.consume { got.add(it.toList()) }

        assertEquals(listOf(listOf<Short>(7, 7)), got)
    }
}
