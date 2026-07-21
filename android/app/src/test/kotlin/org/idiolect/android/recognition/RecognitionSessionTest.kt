package org.idiolect.android.recognition

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The headless state machine for one speech-recognition take — shared by the
 * `ACTION_RECOGNIZE_SPEECH` activity and the system [android.speech.RecognitionService], both of
 * which run with no IME view. The Android wiring (core/audio/router) is injected as [TakeControl]
 * and [RecognitionOutput] so the sequencing and the emit-exactly-once contract are unit-tested
 * with fakes: a caller must never get two results, a duplicate/late commit must be ignored, and
 * an empty transcript must read as "no speech" rather than an empty result the host app pastes.
 */
class RecognitionSessionTest {
    private class FakeTake : TakeControl {
        var starts = 0
        var stops = 0
        var cancels = 0
        override fun start() { starts++ }
        override fun stop() { stops++ }
        override fun cancel() { cancels++ }
    }

    private class Out : RecognitionOutput {
        val events = mutableListOf<String>()
        override fun onReadyForSpeech() { events += "ready" }
        override fun onResult(text: String) { events += "result:$text" }
        override fun onError(error: RecognitionError) { events += "error:$error" }
    }

    @Test
    fun start_opens_the_take_and_announces_ready() {
        val take = FakeTake()
        val out = Out()
        RecognitionSession(take, out).start()
        assertEquals(1, take.starts)
        assertEquals(listOf("ready"), out.events)
    }

    @Test
    fun a_finalized_take_returns_its_transcript_exactly_once() {
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.start()
        session.stopListening()
        session.onCommitted("hello world")
        assertEquals("stopListening finalizes the take", 1, take.stops)
        assertEquals(listOf("ready", "result:hello world"), out.events)
        // A late/duplicate commit from the core must not produce a second result.
        session.onCommitted("hello world")
        assertEquals(listOf("ready", "result:hello world"), out.events)
    }

    @Test
    fun a_blank_transcript_is_a_no_speech_error_not_an_empty_result() {
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.start()
        session.onCommitted("   ")
        assertEquals(listOf("ready", "error:NO_SPEECH"), out.events)
    }

    @Test
    fun the_transcript_is_trimmed() {
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.start()
        session.onCommitted("  hi  ")
        assertEquals(listOf("ready", "result:hi"), out.events)
    }

    @Test
    fun a_core_error_surfaces_once_and_blocks_a_later_commit() {
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.start()
        session.onFailed(RecognitionError.FAILED)
        session.onCommitted("too late")
        assertEquals(listOf("ready", "error:FAILED"), out.events)
    }

    @Test
    fun a_core_failure_discards_the_take_so_capture_never_outlives_the_answer() {
        // A dictationError can fire MID-take (a snippet decode error while capture still runs).
        // The session must discard the take itself: once it is spent, every surface's
        // cancel-before-release teardown is a no-op, so nobody else will stop the mic.
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.start()
        session.onFailed(RecognitionError.FAILED)
        assertEquals("a failed take is discarded", 1, take.cancels)
        assertEquals("never finalized (nothing left to decode)", 0, take.stops)
        assertEquals(listOf("ready", "error:FAILED"), out.events)
    }

    @Test
    fun the_model_load_routing_starts_on_success_and_spends_through_the_session_on_failure() {
        // Pins begin()'s one branch, extracted as routeModelLoad because the surrounding class
        // has no headless seam (native core + Context). Failure must go THROUGH the session:
        // the old direct output.onError left it unspent, so a later stop added a second answer.
        val ok = Out()
        routeModelLoad(loaded = true, session = RecognitionSession(FakeTake(), ok))
        assertEquals(listOf("ready"), ok.events)

        val failed = Out()
        val session = RecognitionSession(FakeTake(), failed)
        routeModelLoad(loaded = false, session = session)
        session.stopListening()
        assertEquals(listOf("error:FAILED"), failed.events)
    }

    @Test
    fun cancel_discards_the_take_and_suppresses_any_result() {
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.start()
        session.cancel()
        session.onCommitted("ignored")
        assertEquals("cancel discards the take", 1, take.cancels)
        // The finalize path decodes the whole take (seconds of whisper work) for a result the
        // session would only suppress — an abandoned take must never pay for it.
        assertEquals("cancel must not finalize", 0, take.stops)
        assertEquals(listOf("ready"), out.events)
    }

    @Test
    fun a_cancel_before_the_take_starts_spends_the_session_without_touching_the_take() {
        // begin() relies on this: a take cancelled while the model is still loading must not
        // open capture when the queued start() finally runs.
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.cancel()
        session.start()
        assertEquals(0, take.starts)
        assertEquals(0, take.cancels)
        assertTrue(out.events.isEmpty())
    }

    @Test
    fun a_stop_before_the_take_starts_answers_no_speech_and_kills_the_queued_start() {
        // begin() relies on this: a stop while the model is still loading must answer the caller
        // NOW (nothing was captured, so NO_SPEECH) and spend the session — otherwise the queued
        // start() would open a mic nobody is left to stop, endless capture after the caller
        // already said stop.
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.stopListening()
        assertEquals(listOf("error:NO_SPEECH"), out.events)
        assertEquals("nothing to finalize before the take starts", 0, take.stops)
        // The queued model-load start arrives after: the spent session must not open capture.
        session.start()
        assertEquals(0, take.starts)
        // And a duplicate stop after the session is spent stays silent (exactly-once).
        session.stopListening()
        assertEquals(listOf("error:NO_SPEECH"), out.events)
    }

    @Test
    fun a_model_load_failure_is_reported_once_and_spends_the_session() {
        // begin() routes a load failure here rather than straight to the output, so the session
        // is spent: a later stop or the (impossible, but defensive) queued start must not produce
        // a second terminal event or open capture.
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.onLoadFailed()
        assertEquals(listOf("error:FAILED"), out.events)
        session.stopListening()
        session.start()
        assertEquals(listOf("error:FAILED"), out.events)
        assertEquals(0, take.starts)
    }

    @Test
    fun a_load_failure_after_cancel_stays_silent() {
        // The caller abandoned the take — SpeechRecognizer.cancel() promises no further callbacks.
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.cancel()
        session.onLoadFailed()
        assertTrue(out.events.isEmpty())
    }

    @Test
    fun a_take_refused_admission_answers_busy_exactly_once() {
        // begin() refuses admission while the shared core is busy with another surface's
        // take (or another headless take holds delivery). The caller hears BUSY now, and
        // exactly once: the spent session drops the queued start and any later stop.
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.onBusy()
        assertEquals(listOf("error:BUSY"), out.events)
        session.start()
        session.stopListening()
        assertEquals(listOf("error:BUSY"), out.events)
        assertEquals("a refused take never opens capture", 0, take.starts)
    }

    @Test
    fun a_synchronous_start_refusal_never_emits_ready_after_the_terminal_answer() {
        // take.start() can refuse reentrantly — an inline executor runs MicToggle's refusal
        // inside start() itself, spending the session before start() returns. The caller must
        // not hear ready after its terminal BUSY (no events follow a terminal answer).
        val out = Out()
        lateinit var session: RecognitionSession
        val take = object : TakeControl {
            override fun start() {
                session.onFailed(RecognitionError.BUSY)
            }
            override fun stop() {}
            override fun cancel() {}
        }
        session = RecognitionSession(take, out)
        session.start()
        assertEquals(listOf("error:BUSY"), out.events)
    }

    @Test
    fun a_busy_refusal_after_cancel_stays_silent() {
        // The caller abandoned the take before admission was decided —
        // SpeechRecognizer.cancel() promises no further callbacks.
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.cancel()
        session.onBusy()
        assertTrue(out.events.isEmpty())
    }

    @Test
    fun a_commit_before_start_is_ignored() {
        val out = Out()
        RecognitionSession(FakeTake(), out).onCommitted("nope")
        assertTrue(out.events.isEmpty())
    }

    @Test
    fun a_silent_finalize_with_no_result_is_reported_as_no_speech() {
        // The core does NOT send commitText/dictationError for a silent take — only
        // recordingStatus(false). Without this, a silent stop would hang the caller forever.
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.start()
        session.onFinalized()
        assertEquals(listOf("ready", "error:NO_SPEECH"), out.events)
    }

    @Test
    fun a_finalize_after_a_result_is_ignored() {
        // For a take WITH speech the core fires commitText before recordingStatus(false), so the
        // session is already spent — the trailing finalize must not turn a good result into an error.
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.start()
        session.onCommitted("hello")
        session.onFinalized()
        assertEquals(listOf("ready", "result:hello"), out.events)
    }

    @Test
    fun a_finalize_after_cancel_is_ignored() {
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)
        session.start()
        session.cancel()
        session.onFinalized()
        assertEquals(listOf("ready"), out.events)
    }
}
