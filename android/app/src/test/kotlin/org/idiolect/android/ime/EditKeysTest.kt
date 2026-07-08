package org.idiolect.android.ime

import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Integration cover for [EditKeys] — the flanking ⌫/⏎ keys of the mic surface (Option A)
 * dispatched onto a [FieldEditor]. Wires the [EnterAction] decision to the field-edit
 * contract using a recording fake (the real `InputConnection` seam is exercised by the
 * emulator e2e). Delete removes the char before the cursor; enter performs the field's IME
 * action or inserts a newline.
 */
class EditKeysTest {
    private class RecordingEditor : FieldEditor {
        val ops = mutableListOf<String>()
        override fun setComposingText(text: String) {}
        override fun commitText(text: String) { ops.add("commit:$text") }
        override fun finishComposingText() {}
        override fun deleteBackward() { ops.add("delete") }
        override fun performEditorAction(actionId: Int) { ops.add("action:$actionId") }
        override fun setSelection(start: Int, end: Int) {}
        override fun fieldText(): String = ""
    }

    @Test
    fun the_delete_key_deletes_the_character_before_the_cursor() {
        val editor = RecordingEditor()
        EditKeys.delete(editor)
        assertEquals(listOf("delete"), editor.ops)
    }

    @Test
    fun the_enter_key_on_a_search_field_performs_the_search_action() {
        val editor = RecordingEditor()
        EditKeys.enter(editor, EditorInfo.IME_ACTION_SEARCH)
        assertEquals(listOf("action:${EditorInfo.IME_ACTION_SEARCH}"), editor.ops)
    }

    @Test
    fun the_enter_key_on_a_plain_field_inserts_a_newline() {
        val editor = RecordingEditor()
        EditKeys.enter(editor, EditorInfo.IME_ACTION_UNSPECIFIED)
        assertEquals(listOf("commit:\n"), editor.ops)
    }

    @Test
    fun the_keys_are_a_no_op_when_no_field_is_focused() {
        // fieldEditor() is null between fields — the keys must not crash.
        EditKeys.delete(null)
        EditKeys.enter(null, EditorInfo.IME_ACTION_SEARCH)
        assertTrue(true)
    }
}
