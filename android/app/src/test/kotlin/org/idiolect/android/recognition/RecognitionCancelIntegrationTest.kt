package org.idiolect.android.recognition

import org.idiolect.android.ime.CaptureControl
import org.idiolect.android.ime.MicToggle
import org.idiolect.android.ime.RecordingToggle
import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.concurrent.Executor

/**
 * Integration of [RecognitionSession] with the real [MicToggle] sequencing — the pairing
 * production uses in [CoreRecognitionTake]. Pins the cancellation contract end to end: abandoning
 * a listening take must reach the core's *discard* (`cancel`), never the finalize `toggle`, whose
 * decode re-transcribes the whole take (seconds of whisper work on the single take executor,
 * which `release()` queues behind) for a result the session would then suppress.
 *
 * The true Android boundary — [CoreRecognitionTake]'s adapter over the native core and a
 * [android.content.Context] — has no headless seam; the connected e2e exercises its one-line
 * delegations (its teardown back-press drives `take.cancel()`) without asserting
 * discard-vs-finalize. This JVM pairing is the deepest gate-runnable level.
 */
class RecognitionCancelIntegrationTest {
    /** Runs each task inline, so ordering assertions stay synchronous. */
    private val direct = Executor { it.run() }

    /** The core + capture call log [MicToggle] sequences (mirrors MicToggleTest's recorder). */
    private class Core : RecordingToggle, CaptureControl {
        val calls = mutableListOf<String>()
        private var recording = false
        override fun isRecording() = recording
        override fun toggle() {
            recording = !recording
            calls.add(if (recording) "core.start" else "core.finalize")
        }
        override fun startContinuous() {
            recording = true
            calls.add("core.startContinuous")
        }
        override fun cancel() {
            recording = false
            calls.add("core.cancel")
        }
        override fun start() {
            calls.add("capture.start")
        }
        override fun stop() {
            calls.add("capture.stop")
        }
    }

    /** Mirrors [CoreRecognitionTake]'s private adapter over [MicToggle]. */
    private class TakeAdapter(private val mic: MicToggle) : TakeControl {
        override fun start() = mic.startHold()
        override fun stop() = mic.stop()
        override fun cancel() = mic.cancel()
    }

    private object DropOutput : RecognitionOutput {
        override fun onReadyForSpeech() {}
        override fun onResult(text: String) {}
        override fun onError(error: RecognitionError) {}
    }

    private fun session(core: Core): RecognitionSession =
        RecognitionSession(TakeAdapter(MicToggle(core, core, direct)), DropOutput)

    @Test
    fun cancelling_a_listening_take_discards_it_in_the_core_without_a_finalize_decode() {
        val core = Core()
        val session = session(core)
        session.start()
        session.cancel()
        assertEquals(
            listOf("core.start", "capture.start", "core.cancel", "capture.stop"),
            core.calls,
        )
    }

    @Test
    fun stopping_normally_still_finalizes_so_the_transcript_can_arrive() {
        val core = Core()
        val session = session(core)
        session.start()
        session.stopListening()
        assertEquals(
            listOf("core.start", "capture.start", "capture.stop", "core.finalize"),
            core.calls,
        )
    }
}
