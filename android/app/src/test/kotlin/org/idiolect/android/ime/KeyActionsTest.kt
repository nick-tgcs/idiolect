package org.idiolect.android.ime

import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The ⌫ / ⏎ keyboard keys drive the field through the [FieldEditor] seam. Pure wiring,
 * host-JVM-testable with a recording fake.
 */
class KeyActionsTest {
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

    @Test
    fun backspace_deletes_the_character_before_the_cursor() {
        val editor = RecordingEditor()
        KeyActions { editor }.backspace()
        assertEquals(listOf("delete"), editor.ops)
    }

    @Test
    fun enter_performs_the_fields_action_when_it_declares_one() {
        val editor = RecordingEditor()
        KeyActions { editor }.enter(EditorInfo.IME_ACTION_SEND)
        assertEquals(listOf("action:${EditorInfo.IME_ACTION_SEND}"), editor.ops)
    }

    @Test
    fun enter_inserts_a_newline_when_the_field_declares_no_action() {
        val editor = RecordingEditor()
        KeyActions { editor }.enter(EditorInfo.IME_ACTION_NONE)
        assertEquals(listOf("commit:\n"), editor.ops)
    }

    @Test
    fun the_keys_are_a_no_op_between_fields_when_there_is_no_editor() {
        // Tapping a key with no focused field must not crash.
        val keys = KeyActions { null }
        keys.backspace()
        keys.enter(EditorInfo.IME_ACTION_SEND)
        assertTrue(true)
    }
}
