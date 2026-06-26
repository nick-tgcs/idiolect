package org.idiolect.android.recognition

import android.content.Context
import org.idiolect.android.audio.AndroidPcmSource
import org.idiolect.android.core.IdiolectCoreHost
import org.idiolect.android.core.NoopInputMethod
import org.idiolect.android.ime.CoreRecordingToggle
import org.idiolect.android.ime.DictationController
import org.idiolect.android.ime.MicToggle
import org.idiolect.android.model.InstalledModel
import org.idiolect.ffi.IdiolectInputMethod
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Wires the process-wide whisper core to a [RecognitionSession] for a **headless** take — the
 * shared engine behind the `ACTION_RECOGNIZE_SPEECH` activity and the system
 * [android.speech.RecognitionService]. It binds the core's commit/error callbacks to the session
 * and drives capture through the same [MicToggle] path the keyboard uses.
 *
 * **Threading is load-bearing.** Every native-core touch — the model load, the [MicToggle]
 * start/stop sequence, and the final core close — runs on the SAME single-thread executor, so they
 * are strictly ordered and no call can ever race the close. (An earlier version loaded on a side
 * thread and closed the core on the main thread, which let a queued `isRecording` land on an
 * already-closed core: "IdiolectCore object has already been destroyed" — caught by the e2e.)
 *
 * The orchestration *logic* — emit-exactly-once, blank-is-no-speech — is the unit-tested
 * [RecognitionSession]; this is the Android wiring around it, covered by the connected e2e.
 */
class CoreRecognitionTake(context: Context) {
    private val host = IdiolectCoreHost.acquire(context.applicationContext)
    private val core = host.core

    // Daemon so a stray idle worker never keeps the process alive; single-thread so the load,
    // the capture sequence, and the close are serialized (see the class note).
    private val executor = Executors.newSingleThreadExecutor { r ->
        Thread(r, "idiolect-recognize").apply { isDaemon = true }
    }
    private val controller = DictationController(
        sink = { frame -> core.pushPcmFrame(frame) },
        sourceFactory = { AndroidPcmSource() },
    )
    private val mic = MicToggle(CoreRecordingToggle(core), controller, executor)

    @Volatile
    private var session: RecognitionSession? = null

    @Volatile
    private var sink: IdiolectInputMethod? = null

    private val released = AtomicBoolean(false)

    /**
     * Begin a take for [model], reporting to [output]. The model load is queued on the executor;
     * once it succeeds the session opens the mic (and only then does [RecognitionOutput.onReadyForSpeech]
     * fire, so a caller never prompts the user to speak before idiolect is listening). A load
     * failure is reported straight to [output] — the session hasn't entered its listening state.
     */
    fun begin(model: InstalledModel, output: RecognitionOutput) {
        val live = RecognitionSession(TakeAdapter(), output)
        session = live
        val callbacks = object : NoopInputMethod() {
            override fun commitText(text: String) = live.onCommitted(text)
            override fun dictationError(message: String) = live.onFailed(RecognitionError.FAILED)
        }
        sink = callbacks
        host.router.bind(callbacks)
        runCatching {
            executor.execute {
                // If the take was cancelled before the load finished, live.start() is a no-op
                // (the session is already spent), so no capture is opened on a doomed take.
                val loaded = runCatching { core.loadModelVerified(model.path, model.sha256) }.isSuccess
                if (loaded) live.start() else output.onError(RecognitionError.FAILED)
            }
        }
    }

    /** End of input — finalize the take; the transcript arrives via the bound callback. */
    fun stopListening() {
        if (!released.get()) session?.stopListening()
    }

    /** Abandon the take with no result. */
    fun cancel() {
        if (!released.get()) session?.cancel()
    }

    /** Unbind and close the core — ordered behind any queued load/capture task on the executor, so
     *  the close never races a native call. Idempotent: the core reference is dropped exactly once. */
    fun release() {
        if (!released.compareAndSet(false, true)) return
        sink?.let { host.router.unbind(it) }
        runCatching { executor.execute { IdiolectCoreHost.release() } }
        executor.shutdown()
    }

    private inner class TakeAdapter : TakeControl {
        override fun start() = mic.startHold()
        override fun stop() = mic.stop()
    }
}
