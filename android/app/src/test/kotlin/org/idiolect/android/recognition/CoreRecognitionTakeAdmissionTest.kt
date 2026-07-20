package org.idiolect.android.recognition

import org.idiolect.android.core.CoreCallbackRouter
import org.idiolect.android.core.NoopInputMethod
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The admission rule for a headless take over the PROCESS-WIDE core, extracted from
 * [CoreRecognitionTake.begin] as [admitTake] / [onStartRefused] because the surrounding class has
 * no headless seam (native core + Context). Before admission existed, `begin()` installed the
 * router override unconditionally: a second RECOGNIZE_SPEECH / RecognitionService request arriving
 * while another surface's take was recording stole that take's commit and finalize callbacks — the
 * original caller hung on "Transcribing…" and the newcomer received the other surface's transcript.
 */
class CoreRecognitionTakeAdmissionTest {
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
    fun an_already_recording_core_refuses_admission_without_claiming_delivery() {
        // The claim must not even be attempted: holding the override while a foreign
        // take is live would swallow that take's pushes (its commits would route to
        // this refused session and be dropped — the other surface's text lost).
        var claimed = false
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)

        val admitted = admitTake(
            coreRecording = true,
            claimDelivery = {
                claimed = true
                true
            },
            session = session,
        )

        assertFalse(admitted)
        assertFalse("delivery must not be claimed for a refused take", claimed)
        assertEquals(listOf("error:BUSY"), out.events)
    }

    @Test
    fun a_take_that_cannot_claim_delivery_is_refused_as_busy() {
        // Two headless takes raced begin(): the router's single delivery slot is the
        // atomic arbiter, and the loser answers BUSY instead of stealing the winner's
        // callbacks.
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)

        assertFalse(admitTake(coreRecording = false, claimDelivery = { false }, session = session))

        assertEquals(listOf("error:BUSY"), out.events)
    }

    @Test
    fun a_free_core_and_delivery_slot_admit_the_take_silently() {
        val out = Out()
        val session = RecognitionSession(FakeTake(), out)

        assertTrue(admitTake(coreRecording = false, claimDelivery = { true }, session = session))

        assertTrue("admission itself reports nothing — the load/start path speaks", out.events.isEmpty())
    }

    @Test
    fun a_refused_capture_start_releases_delivery_and_answers_busy() {
        // Admission raced the IME: between the admit check and the capture start, the IME
        // opened its own take on the shared core, so MicToggle refused ours. Holding the
        // override any longer would swallow the IME's commits; the caller must hear BUSY
        // (not hang waiting on a capture that never started).
        val router = CoreCallbackRouter()
        val sink = object : NoopInputMethod() {
            var commits = 0
            override fun commitText(text: String) { commits++ }
        }
        assertTrue(router.tryAcquireOverride(sink))
        val take = FakeTake()
        val out = Out()
        val session = RecognitionSession(take, out)
        session.start()

        onStartRefused(router, sink, session)

        assertEquals(listOf("ready", "error:BUSY"), out.events)
        assertEquals("the refused take is discarded, never finalized", 1, take.cancels)
        assertEquals(0, take.stops)
        // The override is gone: the IME's own pushes flow to its base binding again.
        router.commitText("keyboard text")
        assertEquals("a released override must not swallow the IME's commit", 0, sink.commits)
    }
}
