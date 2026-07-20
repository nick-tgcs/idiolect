package org.idiolect.android.recognition

import org.idiolect.android.ime.CaptureControl
import org.idiolect.android.ime.MicToggle
import org.idiolect.android.ime.RecordingToggle
import org.junit.Assert.assertEquals
import org.junit.Test
import java.util.concurrent.Executor

/**
 * Integration of [RecognitionSession] with the real [MicToggle] sequencing — the pairing
 * production uses in [CoreRecognitionTake]. Pins the abandon-a-take contracts end to end:
 * cancelling a listening take must reach the core's *discard* (`cancel`), never the finalize
 * `toggle`, whose decode re-transcribes the whole take (seconds of whisper work on the single
 * take executor, which `release()` queues behind) for a result the session would then suppress;
 * and a stop before the take starts must never touch the core at all.
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

    /** Mirrors [CoreRecognitionTake]'s private adapter over [MicToggle], refusal hook included:
     *  unset (the default, as before `begin()` wires it) models the gap before the refusal task
     *  runs; set, it busy-fails the session the way `begin()` does (production additionally
     *  releases the router override first — that half is pinned in
     *  [CoreRecognitionTakeAdmissionTest]). */
    private class TakeAdapter(private val mic: MicToggle) : TakeControl {
        var onRefused: () -> Unit = {}
        override fun start() = mic.startHold(onRefused = onRefused)
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
    fun a_stop_before_the_take_starts_never_opens_capture() {
        // A stop while the model is still loading (the session hasn't started the take): the
        // caller is answered NO_SPEECH and the queued start must find the session spent —
        // through the real MicToggle pairing neither the core nor capture is ever touched, so
        // no mic is left open with nobody to stop it.
        val core = Core()
        val heard = mutableListOf<String>()
        val out = object : RecognitionOutput {
            override fun onReadyForSpeech() { heard += "ready" }
            override fun onResult(text: String) { heard += "result:$text" }
            override fun onError(error: RecognitionError) { heard += "error:$error" }
        }
        val session = RecognitionSession(TakeAdapter(MicToggle(core, core, direct)), out)
        session.stopListening()
        session.start()
        assertEquals(emptyList<String>(), core.calls)
        assertEquals(listOf("error:NO_SPEECH"), heard)
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

    @Test
    fun a_mid_take_core_failure_discards_capture_through_the_real_pairing() {
        // dictationError with capture still running: the session's discard must reach the
        // core's cancel and stop capture — the spent session makes every surface's later
        // cancel-before-release a no-op, so this is the only cleanup the take will get.
        val core = Core()
        val session = session(core)
        session.start()
        session.onFailed(RecognitionError.FAILED)
        assertEquals(
            listOf("core.start", "capture.start", "core.cancel", "capture.stop"),
            core.calls,
        )
    }

    @Test
    fun a_start_that_finds_the_core_taken_busy_fails_through_the_refusal_hook() {
        // Admission won while the core was idle, then the IME opened its own take before our
        // capture start ran: startHold's executor-confined check refuses, and the wired refusal
        // (as begin() installs it) must answer the caller BUSY — never leave it LISTENING on a
        // capture that never opened — while the ownership-gated discard leaves the IME's take
        // untouched.
        val core = Core()
        MicToggle(core, core, direct).onTap() // the IME's take on the shared, process-wide core
        val heard = mutableListOf<String>()
        val out = object : RecognitionOutput {
            override fun onReadyForSpeech() { heard += "ready" }
            override fun onResult(text: String) { heard += "result:$text" }
            override fun onError(error: RecognitionError) { heard += "error:$error" }
        }
        val adapter = TakeAdapter(MicToggle(core, core, direct))
        val session = RecognitionSession(adapter, out)
        adapter.onRefused = { session.onFailed(RecognitionError.BUSY) }
        session.start()
        // With the inline executor the refusal runs INSIDE start(), so the session suppresses
        // the ready that would otherwise trail the terminal answer. On the production executor
        // the refusal is a later task: the caller hears ready, then BUSY — either way nothing
        // follows the terminal event.
        assertEquals(listOf("error:BUSY"), heard)
        assertEquals(
            "the foreign take must survive the refusal's gated discard",
            listOf("core.start", "capture.start"),
            core.calls,
        )
    }

    @Test
    fun a_take_spent_while_its_start_is_still_queued_never_opens_capture() {
        // The start crosses an executor hop. If the session is spent inside that hop —
        // a foreign take's commit routed through the held override, or the caller's
        // instant cancel — the queued startSequence must NOT run: it would open a take
        // and capture that no one is left to stop (the spent session makes every
        // surface's cancel-before-release a no-op) — a hot mic until the IME's next
        // tap finalizes ambient audio into the user's field. CoreRecognitionTake wires
        // the toggle's start gate to the live session for exactly this.
        val core = Core()
        val queued = ArrayDeque<Runnable>()
        val deferred = Executor { queued.add(it) }
        lateinit var session: RecognitionSession
        val mic = MicToggle(core, core, deferred, canStart = { session.isListening() })
        val adapter = TakeAdapter(mic)
        session = RecognitionSession(adapter, DropOutput)
        session.start() // queues the hold-start on the executor
        session.onCommitted("a foreign take's transcript") // spends the session in the hop
        while (queued.isNotEmpty()) queued.removeFirst().run()
        assertEquals(
            "a spent session's queued start must refuse, not open an ownerless capture",
            emptyList<String>(),
            core.calls,
        )
    }

    @Test
    fun a_misrouted_failure_while_a_foreign_take_records_does_not_kill_it() {
        // The router override sends ALL core callbacks to a live recognition session. In
        // production, admission refuses a busy core up front — but the IME can still grab the
        // core after admission won, and its failure can land in the gap between our start and
        // the queued startHold refusal (this adapter's hook left unset models that gap: we are
        // LISTENING without owning the take). The discard in onFailed must not reach the foreign
        // take: MicToggle only cancels what it started. The IME user's in-flight dictation
        // survives; this session is spent and answers its caller once.
        val core = Core()
        MicToggle(core, core, direct).onTap() // the IME's take on the shared, process-wide core
        val session = session(core)
        session.start() // LISTENING, but the busy core means no capture of ours ever opens
        session.onFailed(RecognitionError.FAILED)
        assertEquals(listOf("core.start", "capture.start"), core.calls)
    }
}
