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
class CoreRecognitionTake(context: Context) : RecognitionTake {
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
    // Ephemeral: the recognition surface has no EditorInfo, so it can't detect a password/PIN
    // field — the take is transcription-only and the core persists nothing (no history, source
    // audio, or training pair), so a secret spoken here can't leak into learning/sync.
    private val mic = MicToggle(CoreRecordingToggle(core, ephemeral = true), controller, executor)

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
    override fun begin(model: InstalledModel, output: RecognitionOutput) {
        val live = RecognitionSession(TakeAdapter(), output)
        session = live
        val callbacks = object : NoopInputMethod() {
            override fun commitText(text: String) = live.onCommitted(text)
            override fun dictationError(message: String) = live.onFailed(RecognitionError.FAILED)

            // The core fires recordingStatus(false) LAST on every finalize and is the ONLY signal
            // for a silent take (no commitText/dictationError) — so a silent stop ends as NO_SPEECH
            // instead of hanging the caller. A speech take has already committed by now, so this
            // is a no-op there (the session is spent).
            override fun recordingStatus(recording: Boolean) {
                if (!recording) live.onFinalized()
            }
        }
        sink = callbacks
        // Take delivery ABOVE the IME's base binding for the life of the take, so an IME that is
        // (re)created and binds itself mid-take can't steal this take's finalize callback.
        host.router.acquireOverride(callbacks)
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
    override fun stopListening() {
        if (!released.get()) session?.stopListening()
    }

    /** Abandon the take with no result. */
    override fun cancel() {
        if (!released.get()) session?.cancel()
    }

    /** Drop the override and close the core, BOTH ordered behind any queued load/capture/finalize
     *  task on the executor — so the close never races a native call, and a still-decoding cancelled
     *  take's commit lands on this (spent) session rather than leaking to the IME's base sink.
     *  Idempotent: the core reference is dropped exactly once. */
    override fun release() {
        if (!released.compareAndSet(false, true)) return
        runCatching {
            executor.execute {
                sink?.let { host.router.releaseOverride(it) }
                IdiolectCoreHost.release()
            }
        }
        executor.shutdown()
    }

    private inner class TakeAdapter : TakeControl {
        override fun start() = mic.startHold()
        override fun stop() = mic.stop()
    }
}
