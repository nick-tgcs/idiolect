package org.idiolect.android.audio

/**
 * Reads [source] in fixed-size buffers and offers each captured chunk to [queue]
 * until the source stops, then closes the queue. [run] is the capture thread's whole
 * job: a tight read loop with no decode work on it (see [PcmFrameQueue]).
 */
class AudioCapture(
    private val source: PcmSource,
    private val queue: PcmFrameQueue,
    private val bufferSamples: Int = DEFAULT_BUFFER_SAMPLES,
) {
    /**
     * Start the source, relay each read into the queue, and — always, even on error —
     * stop the source and close the queue so the consumer terminates.
     */
    fun run() {
        source.start()
        try {
            val buffer = ShortArray(bufferSamples)
            while (true) {
                val read = source.read(buffer)
                if (read <= 0) break
                // Copy exactly what was read so a short read never relays stale tail.
                queue.offer(buffer.copyOf(read))
            }
        } finally {
            source.stop()
            queue.close()
        }
    }

    companion object {
        /**
         * 100 ms at 16 kHz. Comfortably larger than the 30 ms VAD frame the core works
         * in, and small enough to keep live preedits responsive; well within
         * `AudioRecord`'s internal buffer so a read never has to wait long.
         */
        const val DEFAULT_BUFFER_SAMPLES = 1_600
    }
}
