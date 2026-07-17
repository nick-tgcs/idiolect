package org.idiolect.android.ime

import java.util.concurrent.Executor

/** The core's recording state + edge toggle (`IdiolectCore.isRecording`/`toggle`). */
interface RecordingToggle {
    fun isRecording(): Boolean
    fun toggle()

    /** Begin a continuous take (the mic's double-tap): `IdiolectCore.startContinuous`. */
    fun startContinuous()

    /** Discard the current take without finalizing (`IdiolectCore.cancel`): nothing persists. */
    fun cancel()
}

/** Start/stop of a dictation take's capture (satisfied by [DictationController]). */
interface CaptureControl {
    fun start()
    fun stop()
}

/**
 * The one-tap mic key. The core is the authority on recording state; this just
 * sequences the capture lifecycle around the core's edge toggle so no audio is lost:
 *
 *  - start: toggle the core on (it begins accepting frames), *then* begin capture;
 *  - stop: stop + drain capture *first* (every captured frame is pushed while the core
 *    still accepts it), *then* toggle the core to finalize the take.
 *
 * The whole sequence runs on [executor] — a single background thread in production —
 * never on the caller's (UI) thread: the finalize `toggle` re-transcribes the entire
 * take, which is seconds of whisper work that would block the IME's UI thread and trip
 * Android's ANR watchdog. A single-thread executor also serialises taps, so the
 * `isRecording` edge check and the sequence it guards stay consistent under rapid taps.
 * The core's own push callbacks already marshal their UI work back to the main thread.
 */
class MicToggle(
    private val core: RecordingToggle,
    private val capture: CaptureControl,
    private val executor: Executor,
    // Gate on *starting* a take. Returns false to refuse (a password/PIN field): the secret take
    // must never reach the core, which would persist its audio, history and a training row.
    // Stopping is never gated — a running take must always finalize cleanly.
    private val canStart: () -> Boolean = { true },
) {
    /** Whether the take currently running on the core was started by THIS toggle. The core is
     *  process-wide (the IME and the recognition surfaces share it), and cancel() is destructive
     *  — discarding another surface's take throws away that user's in-flight dictation — so only
     *  the owner may cancel. Confined to the executor thread. Best-effort: a foreign surface
     *  finalizing our take leaves it stale until the next start (the documented cross-surface
     *  residual); stop/finalize stays unguarded, as a finalize loses nothing. */
    private var ownsTake = false

    /** Single tap: toggle — start a one-shot take if idle, stop + finalize if recording. */
    fun onTap() = submit {
        when {
            core.isRecording() -> stopSequence()
            canStart() -> startSequence()
        }
    }

    /** Press-and-hold begins: ensure a take is recording (idempotent under rapid edges). */
    fun startHold() = submit {
        if (!core.isRecording() && canStart()) startSequence()
    }

    /** Hold released, or a stop tap: ensure the take is stopped and finalized. */
    fun stop() = submit {
        // No take to stop ⇒ no ownership to claim: drop a stale flag left by a foreign surface
        // finalizing our take, so a later cancel() can't discard a take we didn't start.
        if (core.isRecording()) stopSequence() else ownsTake = false
    }

    /**
     * Abort the current take without finalizing — discard it in the core, then drain capture;
     * nothing is decoded or persisted. Used when focus lands on a learning-blocked field while
     * a take (typically continuous) is still recording (a finalize there would persist audio,
     * history and a training row), and when a recognition caller abandons its take (back-press,
     * `SpeechRecognizer.cancel()`, host teardown), where a finalize would burn a whole whisper
     * decode on a result the session only suppresses. Only acts on a take THIS toggle started
     * ([ownsTake]): a misrouted failure or a teardown while a foreign surface's take records
     * must not destroy that user's dictation.
     */
    fun cancel() = submit {
        if (ownsTake && core.isRecording()) {
            // Cancel the core FIRST so it stops accepting and finalizing frames, THEN drain
            // capture — the pump's remaining pushes are rejected (NoActiveTake) and harmlessly
            // dropped, so no blocked-field speech is committed. This is the *opposite* order to
            // stop(), which must drain-then-finalize so a real take loses no audio.
            //
            // core.cancel() throws NoActiveTake if a stop on another surface (the shared core is
            // process-wide — the recognition service drives it too) raced our isRecording() check;
            // swallow it so the executor thread never dies. Capture is still stopped either way.
            runCatching { core.cancel() }
            capture.stop()
        }
        ownsTake = false
    }

    /** Double-tap: begin a continuous take (ignored if one is already running or refused). */
    fun startContinuous() = submit {
        if (!core.isRecording() && canStart()) {
            core.startContinuous()
            capture.start()
            ownsTake = true
        }
    }

    /**
     * Run [task] on the executor, tolerating a shut-down executor. During teardown a late gesture
     * timer can fire after `toggleExecutor.shutdown()`; the rejected task is a harmless no-op, not
     * an uncaught `RejectedExecutionException` that would crash the app.
     */
    private fun submit(task: () -> Unit) {
        runCatching { executor.execute(task) }
    }

    private fun startSequence() {
        core.toggle()
        capture.start()
        ownsTake = true
    }

    private fun stopSequence() {
        capture.stop()
        core.toggle()
        ownsTake = false
    }
}
