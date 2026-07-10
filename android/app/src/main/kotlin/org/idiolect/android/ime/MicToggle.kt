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
    /** Single tap: toggle — start a one-shot take if idle, stop + finalize if recording. */
    fun onTap() {
        executor.execute {
            when {
                core.isRecording() -> stopSequence()
                canStart() -> startSequence()
            }
        }
    }

    /** Press-and-hold begins: ensure a take is recording (idempotent under rapid edges). */
    fun startHold() {
        executor.execute {
            if (!core.isRecording() && canStart()) startSequence()
        }
    }

    /** Hold released, or a stop tap: ensure the take is stopped and finalized. */
    fun stop() {
        executor.execute {
            if (core.isRecording()) stopSequence()
        }
    }

    /**
     * Abort the current take without finalizing — drain capture, then discard the take in the
     * core so nothing is persisted. Used when focus lands on a learning-blocked field while a
     * take (typically continuous) is still recording: a finalize there would persist audio,
     * history and a training row for speech captured in the blocked field.
     */
    fun cancel() {
        executor.execute {
            if (core.isRecording()) {
                capture.stop()
                core.cancel()
            }
        }
    }

    /** Double-tap: begin a continuous take (ignored if one is already running or refused). */
    fun startContinuous() {
        executor.execute {
            if (!core.isRecording() && canStart()) {
                core.startContinuous()
                capture.start()
            }
        }
    }

    private fun startSequence() {
        core.toggle()
        capture.start()
    }

    private fun stopSequence() {
        capture.stop()
        core.toggle()
    }
}
