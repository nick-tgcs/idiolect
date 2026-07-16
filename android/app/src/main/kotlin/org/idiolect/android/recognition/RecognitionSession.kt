package org.idiolect.android.recognition

import org.idiolect.android.model.InstalledModel

/** Start/stop of one recognition take's capture + core finalize, injected so the
 *  [RecognitionSession] sequencing is unit-tested. In production this is a thin adapter over the
 *  IME's [org.idiolect.android.ime.MicToggle] (`startHold` / `stop`). */
interface TakeControl {
    fun start()
    fun stop()
}

/** The service-facing lifecycle of one whisper take ([CoreRecognitionTake] in production). A seam
 *  so the hosting services' teardown ordering — cancel a possibly-live capture BEFORE releasing
 *  the core — is unit-testable without the native core. */
interface RecognitionTake {
    fun begin(model: InstalledModel, output: RecognitionOutput)
    fun stopListening()
    fun cancel()
    fun release()
}

/** Where a take's outcome goes: the `ACTION_RECOGNIZE_SPEECH` activity returns it as
 *  `EXTRA_RESULTS`; the [android.speech.RecognitionService] forwards it to its `Callback`. */
interface RecognitionOutput {
    fun onReadyForSpeech()
    fun onResult(text: String)
    fun onError(error: RecognitionError)
}

/** Why a recognition take could not produce a transcript. Mapped to `SpeechRecognizer.ERROR_*`
 *  by the service glue and to a user-facing message by the activity glue. */
enum class RecognitionError {
    /** Mic permission not granted to idiolect. */
    MIC_PERMISSION,

    /** No speech model installed yet (the user hasn't finished onboarding / a download). */
    MODEL_MISSING,

    /** The take ran but finalized to nothing — silence. */
    NO_SPEECH,

    /** The core failed to decode (load/runtime error). */
    FAILED,
}

/**
 * The headless state machine for a single speech-recognition take, shared by both voice surfaces
 * (the `ACTION_RECOGNIZE_SPEECH` activity and the system [android.speech.RecognitionService]).
 *
 * It guarantees a caller hears back **exactly once**: after a result or an error the session is
 * spent, so a late/duplicate `commitText` from the core (which can fire more than once across a
 * finalize) — or a commit that arrives after the user cancelled — is dropped. An empty finalize
 * is reported as [RecognitionError.NO_SPEECH], never as an empty result the host app would paste.
 *
 * All transitions are `@Synchronized` because the core pushes `onCommitted`/`onFailed` on its
 * callback thread while the UI/caller drives `start`/`stopListening`/`cancel`.
 */
class RecognitionSession(
    private val take: TakeControl,
    private val output: RecognitionOutput,
) {
    private enum class State { IDLE, LISTENING, DONE }

    private var state = State.IDLE

    /** Open the mic and tell the caller to speak. Idempotent — a second call is ignored. */
    @Synchronized
    fun start() {
        if (state != State.IDLE) return
        state = State.LISTENING
        take.start()
        output.onReadyForSpeech()
    }

    /** End of input: stop capture so the core finalizes; the transcript arrives via [onCommitted]. */
    @Synchronized
    fun stopListening() {
        if (state != State.LISTENING) return
        take.stop()
    }

    /** Abandon the take (back-press / caller cancel): stop capture and suppress any later result. */
    @Synchronized
    fun cancel() {
        if (state == State.DONE) return
        val wasListening = state == State.LISTENING
        state = State.DONE
        if (wasListening) take.stop()
    }

    /** The core finalized the take with [text] (blank ⇒ silence). Acts only while listening. */
    @Synchronized
    fun onCommitted(text: String) {
        if (state != State.LISTENING) return
        state = State.DONE
        val trimmed = text.trim()
        if (trimmed.isEmpty()) output.onError(RecognitionError.NO_SPEECH) else output.onResult(trimmed)
    }

    /** The core (or model load) reported a failure. Acts only while listening. */
    @Synchronized
    fun onFailed(error: RecognitionError) {
        if (state != State.LISTENING) return
        state = State.DONE
        output.onError(error)
    }

    /**
     * The take finalized — `recordingStatus(false)`, which the core fires LAST and for EVERY
     * outcome (even a silent take, for which it sends no `commitText`/`dictationError`). If a result
     * or error already arrived this is a no-op; otherwise the take was silent (or captured nothing),
     * reported as [RecognitionError.NO_SPEECH] so the caller is never left hanging on "Transcribing…".
     */
    @Synchronized
    fun onFinalized() {
        if (state != State.LISTENING) return
        state = State.DONE
        output.onError(RecognitionError.NO_SPEECH)
    }
}

/** What blocks a recognition take before it can even start; `null` ⇒ good to go. Kept pure so the
 *  precedence (mic before model) is unit-tested rather than buried in the Android glue. */
object RecognitionPreconditions {
    fun blocker(hasMicPermission: Boolean, hasModel: Boolean): RecognitionError? = when {
        !hasMicPermission -> RecognitionError.MIC_PERMISSION
        !hasModel -> RecognitionError.MODEL_MISSING
        else -> null
    }
}
