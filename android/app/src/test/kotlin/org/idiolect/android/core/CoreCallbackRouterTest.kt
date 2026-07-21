package org.idiolect.android.core

import org.idiolect.ffi.IdiolectInputMethod
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The core lives in a process-wide holder (so it survives the IME being torn down on a
 * keyboard switch — see [IdiolectCoreHost]), but its push callbacks must reach whichever
 * IME is *currently* active. [CoreCallbackRouter] is that indirection: the active IME binds
 * itself as the sink; pushes that arrive with no sink bound (between fields, or while the
 * user is editing in another keyboard) are dropped rather than crashing.
 */
class CoreCallbackRouterTest {
    private class RecordingSink(val name: String) : IdiolectInputMethod {
        val ops = mutableListOf<String>()
        override fun recordingStatus(recording: Boolean) { ops.add("$name:recording:$recording") }
        override fun showPreedit(text: String) { ops.add("$name:show:$text") }
        override fun updatePreedit(text: String) { ops.add("$name:update:$text") }
        override fun commitText(text: String) { ops.add("$name:commit:$text") }
        override fun cancelPreedit() { ops.add("$name:cancel") }
        override fun insertText(text: String) { ops.add("$name:insert:$text") }
        override fun editHistory(id: Long, text: String) { ops.add("$name:editHistory:$id:$text") }
        override fun dictationError(message: String) { ops.add("$name:error:$message") }
    }

    @Test
    fun forwards_every_push_to_the_bound_sink() {
        val router = CoreCallbackRouter()
        val sink = RecordingSink("a")
        router.bind(sink)

        router.recordingStatus(true)
        router.showPreedit("he")
        router.updatePreedit("hello")
        router.commitText("hello.")
        router.cancelPreedit()
        router.insertText("old")
        router.editHistory(7L, "past")
        router.dictationError("no model")

        assertEquals(
            listOf(
                "a:recording:true", "a:show:he", "a:update:hello", "a:commit:hello.",
                "a:cancel", "a:insert:old", "a:editHistory:7:past", "a:error:no model",
            ),
            sink.ops,
        )
    }

    @Test
    fun drops_pushes_when_no_sink_is_bound() {
        val router = CoreCallbackRouter()
        // Never bound: a push that arrives between fields must be a safe no-op, not a crash.
        router.commitText("lost")
        router.recordingStatus(false)
        // (nothing to assert beyond "did not throw")
    }

    @Test
    fun unbinding_stops_delivery_to_that_sink() {
        val router = CoreCallbackRouter()
        val sink = RecordingSink("a")
        router.bind(sink)
        router.commitText("kept")
        router.unbind(sink)
        router.commitText("dropped")
        assertEquals(listOf("a:commit:kept"), sink.ops)
    }

    @Test
    fun binding_a_new_sink_replaces_the_old_one() {
        val router = CoreCallbackRouter()
        val first = RecordingSink("first")
        val second = RecordingSink("second")
        router.bind(first)
        router.bind(second) // the new active IME takes over
        router.commitText("x")
        assertEquals(emptyList<String>(), first.ops)
        assertEquals(listOf("second:commit:x"), second.ops)
    }

    @Test
    fun a_stale_unbind_from_a_replaced_sink_does_not_disturb_the_current_one() {
        val router = CoreCallbackRouter()
        val first = RecordingSink("first")
        val second = RecordingSink("second")
        router.bind(first)
        router.bind(second)
        // The old IME tears down late and unbinds itself — must not unbind `second`.
        router.unbind(first)
        router.commitText("x")
        assertEquals(listOf("second:commit:x"), second.ops)
    }

    @Test
    fun an_override_takes_delivery_above_the_bound_base() {
        val router = CoreCallbackRouter()
        val ime = RecordingSink("ime")
        val take = RecordingSink("take")
        router.bind(ime)
        assertTrue(router.tryAcquireOverride(take))
        router.commitText("voice")
        router.recordingStatus(false)
        assertEquals(listOf("take:commit:voice", "take:recording:false"), take.ops)
        assertEquals("the base must hear nothing while an override is active", emptyList<String>(), ime.ops)
    }

    @Test
    fun an_ime_rebinding_mid_take_cannot_steal_the_overrides_callbacks() {
        // The exact hang: while a recognition take holds the override, the IME is (re)created and
        // binds the base — the take's finalize (recordingStatus(false)) must still reach the take,
        // or the caller is stuck on "Transcribing…".
        val router = CoreCallbackRouter()
        val take = RecordingSink("take")
        assertTrue(router.tryAcquireOverride(take))
        val ime = RecordingSink("ime")
        router.bind(ime) // IME created mid-take and grabs the base
        router.recordingStatus(false)
        assertEquals(listOf("take:recording:false"), take.ops)
        assertEquals(emptyList<String>(), ime.ops)
    }

    @Test
    fun a_second_take_cannot_steal_a_held_override() {
        // The slot is the atomic arbiter between concurrent headless takes. Before this,
        // acquire simply replaced the slot: a second RECOGNIZE_SPEECH / RecognitionService
        // request arriving mid-take rerouted the FIRST take's commit and finalize to the
        // newcomer — the original caller hung and the wrong surface received the transcript.
        val router = CoreCallbackRouter()
        val first = RecordingSink("first")
        val second = RecordingSink("second")
        assertTrue(router.tryAcquireOverride(first))
        assertFalse("a held slot must refuse a second take", router.tryAcquireOverride(second))
        router.commitText("dictation")
        assertEquals(listOf("first:commit:dictation"), first.ops)
        assertEquals(emptyList<String>(), second.ops)
        // The refused take's teardown must not evict the holder either.
        router.releaseOverride(second)
        router.recordingStatus(false)
        assertEquals(listOf("first:commit:dictation", "first:recording:false"), first.ops)
    }

    @Test
    fun reacquiring_the_held_override_is_idempotent() {
        val router = CoreCallbackRouter()
        val take = RecordingSink("take")
        assertTrue(router.tryAcquireOverride(take))
        assertTrue("the holder itself may re-claim", router.tryAcquireOverride(take))
        router.commitText("x")
        assertEquals(listOf("take:commit:x"), take.ops)
    }

    @Test
    fun releasing_the_override_falls_back_to_the_bound_base() {
        val router = CoreCallbackRouter()
        val ime = RecordingSink("ime")
        val take = RecordingSink("take")
        router.bind(ime)
        assertTrue(router.tryAcquireOverride(take))
        router.releaseOverride(take)
        router.commitText("kbd")
        assertEquals(listOf("ime:commit:kbd"), ime.ops)
        assertEquals(emptyList<String>(), take.ops)
    }

    @Test
    fun a_late_duplicate_release_does_not_disturb_the_next_takes_override() {
        val router = CoreCallbackRouter()
        val first = RecordingSink("first")
        val second = RecordingSink("second")
        assertTrue(router.tryAcquireOverride(first))
        router.releaseOverride(first)
        assertTrue("a released slot is free for the next take", router.tryAcquireOverride(second))
        router.releaseOverride(first) // a late duplicate release from the finished first take
        router.commitText("x")
        assertEquals(listOf("second:commit:x"), second.ops)
    }
}
