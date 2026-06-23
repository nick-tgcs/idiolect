package org.idiolect.android.audio

import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.atomic.AtomicBoolean

/**
 * A FIFO relay decoupling the `AudioRecord` capture thread (producer) from the core
 * push thread (consumer).
 *
 * Decoupling is not optional: pushing a frame into the core can block for a snippet
 * decode (CPU Whisper, run under the core's lock), and if that ran on the capture
 * thread it would stall `AudioRecord.read`, overrun the mic buffer, and drop samples
 * — i.e. drop *words*, the very failure the streaming pipeline exists to prevent. So
 * the capture thread only ever does the tight read + [offer]; a separate thread runs
 * [consume] and pays the decode cost.
 */
class PcmFrameQueue {
    private val queue = LinkedBlockingQueue<ShortArray>()
    private val closed = AtomicBoolean(false)

    /** Producer: enqueue a captured frame. Ignored once [close]d. */
    fun offer(frame: ShortArray) {
        if (!closed.get()) {
            queue.put(frame)
        }
    }

    /** Producer: signal end of stream. [consume] returns after draining what precedes. */
    fun close() {
        if (closed.compareAndSet(false, true)) {
            queue.put(POISON)
        }
    }

    /**
     * Consumer: deliver queued frames in order on the calling thread, blocking for
     * more, until [close] is observed. Intended to run on a dedicated pump thread.
     */
    fun consume(onFrame: (ShortArray) -> Unit) {
        while (true) {
            val frame = queue.take()
            if (frame === POISON) return
            onFrame(frame)
        }
    }

    private companion object {
        /** Sentinel enqueued by [close]; reference-compared, never delivered. */
        val POISON = ShortArray(0)
    }
}
