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
