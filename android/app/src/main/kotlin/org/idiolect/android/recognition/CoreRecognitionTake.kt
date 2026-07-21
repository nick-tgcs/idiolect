package org.idiolect.android.recognition

import android.content.Context
import androidx.annotation.VisibleForTesting
import org.idiolect.android.audio.AndroidPcmSource
import org.idiolect.android.core.CoreCallbackRouter
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
    // The start gate reads the LIVE session: the start crosses an executor hop, and a session
    // spent inside it (a foreign commit through the held override, an instant cancel) must
    // refuse the queued start — an opened take would have no one left to stop it (hot mic).
    private val mic = MicToggle(
        CoreRecordingToggle(core, ephemeral = true),
        controller,
        executor,
        canStart = { session?.isListening() == true },
    )

    @Volatile
    private var session: RecognitionSession? = null

    @Volatile
    private var sink: IdiolectInputMethod? = null

    private val released = AtomicBoolean(false)

    /**
     * Begin a take for [model], reporting to [output]. Admission, the model load, and the start
     * are all queued on the executor, in that order:
     *
     *  1. **Admission** ([admitTake]): the process-wide core must be free and the router's single
     *     delivery slot claimable — else the session busy-fails NOW. Claiming delivery while
     *     another surface's take runs would reroute ITS commit/finalize here (that caller hangs,
     *     this one receives a foreign transcript), so an overlapping HEADLESS take is refused
     *     atomically at the slot. The IME does not participate in the slot: it can still start on
     *     the shared core inside our model-load window, in which case its early pushes route to
     *     the held override until step 3's refusal releases it (the documented residual).
     *     The claim doubles as delivery ABOVE the IME's base binding for the life of the take, so
     *     an IME that is (re)created and binds itself mid-take can't steal the finalize callback.
     *  2. The model load; a failure goes through [RecognitionSession.onLoadFailed], spending the
     *     session, so the caller hears FAILED exactly once and a later stop cannot add a second
     *     answer.
     *  3. The start — and only then does [RecognitionOutput.onReadyForSpeech] fire, so a caller
     *     never prompts the user to speak before idiolect is listening. If the IME grabbed the
     *     core between admission and the capture start, [MicToggle]'s executor-confined refusal
     *     reports through [onStartRefused]: delivery is released and the session busy-fails
     *     instead of hanging.
     */
    override fun begin(model: InstalledModel, output: RecognitionOutput) {
        val adapter = TakeAdapter()
        val live = RecognitionSession(adapter, output)
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
        adapter.onRefused = { onStartRefused(host.router, callbacks, live) }
        runCatching {
            executor.execute {
                if (!admitTake(
                        coreRecording = core.isRecording(),
                        claimDelivery = { host.router.tryAcquireOverride(callbacks) },
                        session = live,
                    )
                ) {
                    return@execute
                }
                // If the take was cancelled or stopped before the load finished, the session is
                // already spent (a pre-start stop was answered NO_SPEECH on the spot), so
                // starting it is a no-op and no capture is opened on a doomed take.
                val loaded = runCatching { core.loadModelVerified(model.path, model.sha256) }.isSuccess
                routeModelLoad(loaded, live)
            }
        }
    }

    /** End of input — finalize the take so the transcript arrives via the bound callback; before
     *  the take has started (model still loading) the session answers no-speech instead. */
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
        /** Set by [begin] before the executor task runs; invoked on the executor when the
         *  capture start found the core taken (see [onStartRefused]). */
        var onRefused: () -> Unit = {}

        override fun start() = mic.startHold(onRefused = onRefused)
        override fun stop() = mic.stop()
        override fun cancel() = mic.cancel()
    }
}

/** The one branch of [CoreRecognitionTake.begin]'s load task, extracted because the surrounding
 *  class has no headless seam (native core + Context): success starts the session; failure spends
 *  it THROUGH [RecognitionSession.onLoadFailed] — never around it, which would leave the session
 *  unspent so a later stop added a second answer. */
@VisibleForTesting
internal fun routeModelLoad(loaded: Boolean, session: RecognitionSession) {
    if (loaded) session.start() else session.onLoadFailed()
}

/** [CoreRecognitionTake.begin]'s admission rule over the process-wide core, extracted like
 *  [routeModelLoad] (the class has no headless seam). Order is load-bearing: a recording core
 *  refuses BEFORE the delivery claim is attempted — claiming while a foreign take is live would
 *  swallow that take's pushes (its commits would land on this spent session and be dropped).
 *  With the core free, the router's single slot is the atomic arbiter between racing headless
 *  takes: the loser busy-fails instead of stealing the winner's callbacks. */
@VisibleForTesting
internal fun admitTake(
    coreRecording: Boolean,
    claimDelivery: () -> Boolean,
    session: RecognitionSession,
): Boolean {
    if (coreRecording || !claimDelivery()) {
        session.onBusy()
        return false
    }
    return true
}

/** The refusal path of the take's capture start ([MicToggle.startHold]'s executor-confined
 *  "core already taken" answer — admission raced the IME's own start): release the delivery
 *  claim FIRST, so the live take's pushes flow back to the IME's base binding instead of being
 *  swallowed by this spent session, then busy-fail so the caller is answered rather than left
 *  hanging on a capture that never opened. */
@VisibleForTesting
internal fun onStartRefused(
    router: CoreCallbackRouter,
    sink: IdiolectInputMethod,
    session: RecognitionSession,
) {
    router.releaseOverride(sink)
    session.onFailed(RecognitionError.BUSY)
}
