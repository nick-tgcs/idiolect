package org.idiolect.android.audio

/**
 * A source of 16 kHz mono signed-16-bit PCM — the headless-testable seam over
 * `AudioRecord`. The IME supplies an `AudioRecord`-backed implementation; tests
 * supply a scripted one.
 */
interface PcmSource {
    /** Begin capture (configure + start the underlying recorder). */
    fun start()

    /**
     * Read up to `into.size` samples into [into], blocking until some are available.
     * Returns the number of samples read (> 0), or `<= 0` once the source has stopped
     * or errored — at which point the capture loop terminates.
     */
    fun read(into: ShortArray): Int

    /** Stop capture and release the underlying recorder. */
    fun stop()
}
