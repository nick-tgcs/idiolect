package org.idiolect.android.ime

import org.idiolect.android.audio.PcmSource
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit

/**
 * Tests the [DictationController] thread orchestration: on [DictationController.start]
 * a capture thread reads the [PcmSource] and a pump thread relays frames into the
 * core sink; on [DictationController.stop] the source is stopped, both threads wind
 * down, and every captured frame has been delivered.
 *
 * The controller is stopped *before* the finalize toggle (so frames are still
 * accepted) and never joins a thread while the core lock is held — see the class doc.
 */
class DictationControllerTest {
    /**
     * A source that yields a fixed script of reads, signals when the script is drained,
     * then blocks in [read] until [stop] (mirroring AudioRecord, whose blocked read
     * returns when another thread stops it).
     */
    private class BlockingScriptedSource(private val chunks: List<ShortArray>) : PcmSource {
        val started = CountDownLatch(1)
        private val drained = CountDownLatch(1)
        private val stopped = CountDownLatch(1)
        private var index = 0

        override fun start() {
            started.countDown()
        }

        override fun read(into: ShortArray): Int {
            if (index < chunks.size) {
                val chunk = chunks[index++]
                chunk.copyInto(into)
                if (index == chunks.size) drained.countDown()
                return chunk.size
            }
            // Script exhausted: block like a live mic until stopped.
            stopped.await()
            return 0
        }

        override fun stop() {
            stopped.countDown()
        }

        fun awaitDrained() {
            assertTrue("source drained", drained.await(5, TimeUnit.SECONDS))
        }
    }

    @Test
    fun frames_flow_from_source_to_sink_until_stopped() {
        val pushed = CopyOnWriteArrayList<List<Short>>()
        val source = BlockingScriptedSource(listOf(shortArrayOf(1, 2), shortArrayOf(3, 4)))
        val controller = DictationController(
            sink = { frame -> pushed.add(frame) },
            sourceFactory = { source },
        )

        controller.start()
        source.awaitDrained()
        controller.stop() // unblocks read()->0, joins capture + pump

        assertEquals(listOf(listOf<Short>(1, 2), listOf<Short>(3, 4)), pushed.toList())
        assertFalse("not active after stop", controller.isActive())
    }

    @Test
    fun start_is_idempotent_and_only_one_source_is_opened() {
        val opened = CopyOnWriteArrayList<BlockingScriptedSource>()
        val controller = DictationController(
            sink = { },
            sourceFactory = {
                BlockingScriptedSource(listOf(shortArrayOf(1))).also { opened.add(it) }
            },
        )

        controller.start()
        controller.start() // second start while active must be a no-op
        assertTrue("active after start", controller.isActive())
        controller.stop()

        assertEquals("exactly one source opened", 1, opened.size)
    }

    @Test
    fun stop_without_start_is_a_no_op() {
        val controller = DictationController(sink = { }, sourceFactory = { error("unused") })
        controller.stop()
        assertFalse(controller.isActive())
    }

    @Test
    fun a_take_can_be_restarted_after_stop() {
        val pushed = CopyOnWriteArrayList<List<Short>>()
        var nth = 0
        val controller = DictationController(
            sink = { frame -> pushed.add(frame) },
            sourceFactory = {
                nth += 1
                BlockingScriptedSource(listOf(shortArrayOf(nth.toShort())))
            },
        )

        controller.start()
        // Drain + stop the first take, then run a second.
        Thread.sleep(50)
        controller.stop()
        controller.start()
        Thread.sleep(50)
        controller.stop()

        assertEquals(listOf(listOf<Short>(1), listOf<Short>(2)), pushed.toList())
    }
}
