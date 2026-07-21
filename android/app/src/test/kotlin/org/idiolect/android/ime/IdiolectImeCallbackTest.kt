package org.idiolect.android.ime

import org.idiolect.ffi.IdiolectInputMethod
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Unit test for the mapping from the Rust core's [IdiolectInputMethod] callbacks onto
 * the field-editing + UI operations the IME performs. This is pure logic (no Android
 * framework), so it runs on the host JVM: the real `InputConnection` plumbing lives
 * behind [FieldEditor], and the UI surface behind [ImeUiHost].
 */
class IdiolectImeCallbackTest {
    private class RecordingEditor : FieldEditor {
        val ops = mutableListOf<String>()
        override fun setComposingText(text: String) { ops.add("compose:$text") }
        override fun commitText(text: String) { ops.add("commit:$text") }
        override fun finishComposingText() { ops.add("finish") }
        override fun deleteBackward() { ops.add("delete") }
        override fun performEditorAction(actionId: Int) { ops.add("action:$actionId") }
        override fun setSelection(start: Int, end: Int) { ops.add("select:$start:$end") }
        override fun fieldText(): String = ""
    }

    private class RecordingUi(private val review: Boolean = false) : ImeUiHost {
        val ops = mutableListOf<String>()
        override fun onRecordingChanged(recording: Boolean) { ops.add("recording:$recording") }
        override fun onCommit(text: String) { ops.add("commit:$text") }
        override fun isReviewEnabled(): Boolean = review
        override fun onLivePreedit(text: String) { ops.add("live:$text") }
        override fun onReviewRequested(text: String) { ops.add("review:$text") }
        override fun onEditHistory(id: Long, text: String) { ops.add("editHistory:$id:$text") }
        override fun onDictationError(message: String) { ops.add("error:$message") }
    }

    private fun callback(editor: FieldEditor?, ui: ImeUiHost): IdiolectInputMethod =
        IdiolectImeCallback(editorProvider = { editor }, ui = ui)

    @Test
    fun live_preedit_maps_to_set_composing_text_when_review_is_off() {
        val editor = RecordingEditor()
        val ui = RecordingUi(review = false)
        val cb = callback(editor, ui)
        cb.showPreedit("hel")
        cb.updatePreedit("hello")
        // Review off: the live partials type into the host field, as before. (User's rule:
        // "into the target text box / app if review is not enabled".)
        assertEquals(listOf("compose:hel", "compose:hello"), editor.ops)
        assertEquals(emptyList<String>(), ui.ops)
    }

    @Test
    fun live_preedit_streams_to_the_review_surface_not_the_field_when_review_is_on() {
        val editor = RecordingEditor()
        val ui = RecordingUi(review = true)
        val cb = callback(editor, ui)
        cb.showPreedit("hel")
        cb.updatePreedit("hello")
        // Review on: the host field is NEVER touched — the words stream onto idiolect's own
        // review surface. (User's rule: "into the review dialog if it is enabled".)
        assertEquals(emptyList<String>(), editor.ops)
        assertEquals(listOf("live:hel", "live:hello"), ui.ops)
    }

    @Test
    fun commit_and_insert_map_to_commit_text() {
        val editor = RecordingEditor()
        val cb = callback(editor, RecordingUi())
        cb.commitText("done.")
        cb.insertText("snippet")
        assertEquals(listOf("commit:done.", "commit:snippet"), editor.ops)
    }

    @Test
    fun a_take_commit_also_notifies_the_ui_but_a_history_reinsert_does_not() {
        val ui = RecordingUi()
        val cb = callback(RecordingEditor(), ui)
        cb.commitText("the take")
        cb.insertText("old history") // reinsertion is not a fresh take to correct
        assertEquals(listOf("commit:the take"), ui.ops)
    }

    @Test
    fun in_review_mode_a_take_is_routed_to_review_not_typed_into_the_field() {
        val editor = RecordingEditor()
        val ui = RecordingUi(review = true)
        val cb = callback(editor, ui)
        cb.commitText("dictated text")
        // The take is NOT typed and NOT seeded as chips — it goes to the review surface.
        assertEquals(emptyList<String>(), editor.ops)
        assertEquals(listOf("review:dictated text"), ui.ops)
    }

    @Test
    fun a_history_reinsert_still_types_even_in_review_mode() {
        val editor = RecordingEditor()
        val ui = RecordingUi(review = true)
        val cb = callback(editor, ui)
        // insertText is a history reinsertion, not a fresh take — review never intercepts it.
        cb.insertText("old entry")
        assertEquals(listOf("commit:old entry"), editor.ops)
        assertEquals(emptyList<String>(), ui.ops)
    }

    @Test
    fun cancel_preedit_finishes_composing_when_review_is_off() {
        val editor = RecordingEditor()
        val cb = callback(editor, RecordingUi(review = false))
        cb.cancelPreedit()
        assertEquals(listOf("finish"), editor.ops)
    }

    @Test
    fun cancel_preedit_clears_the_review_surface_not_the_field_when_review_is_on() {
        val editor = RecordingEditor()
        val ui = RecordingUi(review = true)
        val cb = callback(editor, ui)
        cb.cancelPreedit()
        // Nothing touches the host field; the live surface is cleared with an empty push.
        assertEquals(emptyList<String>(), editor.ops)
        assertEquals(listOf("live:"), ui.ops)
    }

    @Test
    fun ui_pushes_route_to_the_ui_host() {
        val ui = RecordingUi()
        val cb = callback(RecordingEditor(), ui)
        cb.recordingStatus(true)
        cb.recordingStatus(false)
        cb.editHistory(7L, "past")
        cb.dictationError("no model")
        assertEquals(
            listOf("recording:true", "recording:false", "editHistory:7:past", "error:no model"),
            ui.ops,
        )
    }

    @Test
    fun typing_is_a_no_op_when_no_field_is_focused() {
        val ui = RecordingUi()
        val cb = callback(editor = null, ui = ui)
        // No focused field (editorProvider returns null): typing must not crash.
        cb.showPreedit("x")
        cb.commitText("y")
        cb.cancelPreedit()
        cb.insertText("z")
        // UI pushes are independent of the field and still fire — including the take
        // commit (the field op is the no-op, not the UI notification).
        cb.recordingStatus(true)
        assertEquals(listOf("commit:y", "recording:true"), ui.ops)
    }
}
