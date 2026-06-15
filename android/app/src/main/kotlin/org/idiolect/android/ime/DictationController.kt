package org.idiolect.android.ime

import org.idiolect.android.audio.AudioCapture
import org.idiolect.android.audio.PcmFrameQueue
import org.idiolect.android.audio.PcmSource
import kotlin.concurrent.thread

/** The core's frame intake (`IdiolectCore.pushPcmFrame`), abstracted for testing. */
fun interface PcmSink {
    fun pushPcmFrame(frame: List<Short>)
}

/**
 * Orchestrates a dictation take's two threads: a capture thread that reads the
 * [PcmSource] tightly (via [AudioCapture]) and a pump thread that relays frames from
 * the [PcmFrameQueue] into the core [sink]. The queue decouples them so a blocking
 * snippet decode never stalls the mic.
 *
 * **Lifecycle / deadlock-safety:** the service stops a take by calling [stop] *before*
 * the finalize `toggle` — so every captured frame is delivered while the core is still
 * recording (and accepts it), and [stop] joins the threads with no core lock held.
 * Driving teardown from the core's `recordingStatus(false)` callback instead would
 * deadlock: that callback runs under the core lock, and the pump thread it would join
 * needs that same lock to push.
 */
class DictationController(
    private val sink: PcmSink,
    private val sourceFactory: () -> PcmSource,
) {
    private var capture: Thread? = null
    private var pump: Thread? = null
    private var source: PcmSource? = null

    /** Whether a take is currently capturing. */
    @Synchronized
    fun isActive(): Boolean = source != null

    /** Begin a take: open a source and spawn the capture + pump threads. Idempotent. */
    @Synchronized
    fun start() {
        if (source != null) return
        val queue = PcmFrameQueue()
        val src = sourceFactory()
        source = src
        // The pump pays the (blocking) push cost; a frame that races past the recording
        // window is rejected by the core and harmlessly dropped here.
        pump = thread(name = "idiolect-pcm-pump") {
            queue.consume { frame -> runCatching { sink.pushPcmFrame(frame.toList()) } }
        }
        capture = thread(name = "idiolect-pcm-capture") {
            AudioCapture(src, queue).run()
        }
    }

    /**
     * End the take: stop the source (unblocking the capture read), which closes the
     * queue and drains the pump, then join both threads. Safe to call when idle.
     */
    @Synchronized
    fun stop() {
        val src = source ?: return
        src.stop()
        capture?.join()
        pump?.join()
        capture = null
        pump = null
        source = null
    }
}
