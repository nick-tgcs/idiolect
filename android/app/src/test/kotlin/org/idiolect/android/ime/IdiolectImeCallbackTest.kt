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
        override fun setSelection(start: Int, end: Int) { ops.add("select:$start:$end") }
        override fun fieldText(): String = ""
    }

    private class RecordingUi : ImeUiHost {
        val ops = mutableListOf<String>()
        override fun onRecordingChanged(recording: Boolean) { ops.add("recording:$recording") }
        override fun onCommit(text: String) { ops.add("commit:$text") }
        override fun onEditHistory(id: Long, text: String) { ops.add("editHistory:$id:$text") }
        override fun onDictationError(message: String) { ops.add("error:$message") }
    }

    private fun callback(editor: FieldEditor?, ui: ImeUiHost): IdiolectInputMethod =
        IdiolectImeCallback(editorProvider = { editor }, ui = ui)

    @Test
    fun live_preedit_maps_to_set_composing_text() {
        val editor = RecordingEditor()
        val cb = callback(editor, RecordingUi())
        cb.showPreedit("hel")
        cb.updatePreedit("hello")
        assertEquals(listOf("compose:hel", "compose:hello"), editor.ops)
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
    fun cancel_preedit_finishes_composing() {
        val editor = RecordingEditor()
        val cb = callback(editor, RecordingUi())
        cb.cancelPreedit()
        assertEquals(listOf("finish"), editor.ops)
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
