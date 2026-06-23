package org.idiolect.ffi

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.file.Files

/**
 * The Rust↔Kotlin bridge smoke test: drives the real [IdiolectCore] (real SQLite
 * store under a temp dir, the real CPU streaming pipeline) through the generated
 * UniFFI bindings, proving every marshalling path the IME relies on:
 *
 *  - object construction + the [IdiolectInputMethod] callback interface (Kotlin→Rust
 *    lowering of the callback, Rust→Kotlin dispatch of `recordingStatus`),
 *  - primitive + collection converters (`String`, `Long`, `UInt`, `Boolean`,
 *    `List<Short>` PCM frames, `List<HistoryItem>`),
 *  - typed error lifting (`FfiException.HistoryEntryNotFound`).
 *
 * It deliberately does NOT assert audio/VAD/Whisper semantics — those are covered by
 * the Rust seam tests against the same code; here the contract under test is the FFI
 * boundary itself.
 */
class BridgeTest {
    /** Records every callback the core pushes, in order. */
    private class RecordingCallback : IdiolectInputMethod {
        val events = mutableListOf<String>()
        override fun recordingStatus(recording: Boolean) = add("recording:$recording")
        override fun showPreedit(text: String) = add("show:$text")
        override fun updatePreedit(text: String) = add("update:$text")
        override fun commitText(text: String) = add("commit:$text")
        override fun cancelPreedit() = add("cancelPreedit")
        override fun insertText(text: String) = add("insert:$text")
        override fun editHistory(id: Long, text: String) = add("editHistory:$id:$text")
        override fun dictationError(message: String) = add("error:$message")
        private fun add(event: String) {
            events.add(event)
        }
    }

    private fun newCore(callback: IdiolectInputMethod): IdiolectCore {
        val dir = Files.createTempDirectory("idiolect-bridge").toFile()
        return IdiolectCore(dir.absolutePath, null, callback)
    }

    @Test
    fun the_recording_lifecycle_round_trips_through_the_ffi() {
        val callback = RecordingCallback()
        newCore(callback).use { core ->
            assertFalse("starts idle", core.isRecording())

            core.toggle() // start
            assertTrue("recording after first toggle", core.isRecording())

            // ~0.5 s of 16 kHz mono PCM, pushed as 30 ms (480-sample) frames — proves
            // the `List<Short>` hot path marshals without error.
            val frame = List(480) { 0.toShort() }
            repeat(16) { core.pushPcmFrame(frame) }

            core.toggle() // stop
            assertFalse("idle after second toggle", core.isRecording())
        }

        assertTrue("start was pushed", callback.events.contains("recording:true"))
        assertTrue("stop was pushed", callback.events.contains("recording:false"))
        // Silence persists nothing, so nothing was committed to the field.
        assertTrue("no commit on silence", callback.events.none { it.startsWith("commit:") })
    }

    @Test
    fun an_empty_store_has_no_history() {
        newCore(RecordingCallback()).use { core ->
            val history: List<HistoryItem> = core.recentHistory(10u)
            assertEquals("fresh store is empty", 0, history.size)
        }
    }

    @Test
    fun a_history_key_marshals_and_a_wrong_length_lifts_a_typed_error() {
        val dir = Files.createTempDirectory("idiolect-key").toFile()
        // A 32-byte ByteArray? key marshals across the FFI and opens the store.
        IdiolectCore(dir.absolutePath, ByteArray(32) { 7 }, RecordingCallback()).use { core ->
            assertEquals(0, core.recentHistory(10u).size)
        }
        // A wrong-length key surfaces the typed error, not a panic across the boundary.
        assertThrows(FfiException.InvalidHistoryKey::class.java) {
            IdiolectCore(dir.absolutePath, ByteArray(16), RecordingCallback())
        }
    }

    @Test
    fun editing_a_missing_history_entry_throws_a_typed_error() {
        newCore(RecordingCallback()).use { core ->
            assertThrows(FfiException.HistoryEntryNotFound::class.java) {
                core.historyEdited(424_242L, "no such row")
            }
            assertThrows(FfiException.HistoryEntryNotFound::class.java) {
                core.reinsertHistory(424_242L)
            }
        }
    }
}
