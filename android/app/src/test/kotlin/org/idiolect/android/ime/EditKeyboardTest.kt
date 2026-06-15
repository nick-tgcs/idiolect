package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The edit-mode key reducer: maps key taps onto [FieldEditor] operations, holding the
 * one-shot shift state. Pure logic (a fake [FieldEditor] records the ops), so it runs
 * on the host JVM; the key rendering is the GUI seam.
 */
class EditKeyboardTest {
    private class RecordingEditor : FieldEditor {
        val ops = mutableListOf<String>()
        override fun setComposingText(text: String) { ops.add("compose:$text") }
        override fun commitText(text: String) { ops.add("commit:$text") }
        override fun finishComposingText() { ops.add("finish") }
        override fun deleteBackward() { ops.add("delete") }
        override fun setSelection(start: Int, end: Int) { ops.add("select:$start:$end") }
        override fun fieldText(): String = ""
    }

    private fun a() = Key.Character("a", "A")
    private fun b() = Key.Character("b", "B")

    @Test
    fun a_character_commits_lower_case_by_default() {
        val editor = RecordingEditor()
        val kb = EditKeyboard(editor = { editor }, onSwitchToVoice = {})
        kb.onKey(a())
        assertEquals(listOf("commit:a"), editor.ops)
        assertFalse(kb.isShifted)
    }

    @Test
    fun shift_upper_cases_the_next_character_then_resets() {
        val editor = RecordingEditor()
        val kb = EditKeyboard(editor = { editor }, onSwitchToVoice = {})
        kb.onKey(Key.Shift)
        assertTrue(kb.isShifted)
        kb.onKey(a())
        kb.onKey(b())
        assertEquals(listOf("commit:A", "commit:b"), editor.ops)
        assertFalse(kb.isShifted)
    }

    @Test
    fun backspace_space_and_enter_map_to_field_ops() {
        val editor = RecordingEditor()
        val kb = EditKeyboard(editor = { editor }, onSwitchToVoice = {})
        kb.onKey(Key.Backspace)
        kb.onKey(Key.Space)
        kb.onKey(Key.Enter)
        assertEquals(listOf("delete", "commit: ", "commit:\n"), editor.ops)
    }

    @Test
    fun switch_to_voice_fires_the_callback_not_the_field() {
        val editor = RecordingEditor()
        var switched = 0
        val kb = EditKeyboard(editor = { editor }, onSwitchToVoice = { switched++ })
        kb.onKey(Key.SwitchToVoice)
        assertEquals(1, switched)
        assertTrue(editor.ops.isEmpty())
    }

    @Test
    fun typing_is_a_no_op_when_no_field_is_focused() {
        var switched = 0
        val kb = EditKeyboard(editor = { null }, onSwitchToVoice = { switched++ })
        // No focused field: field keys must not crash.
        kb.onKey(a())
        kb.onKey(Key.Backspace)
        kb.onKey(Key.Space)
        // Shift state and the mode switch are field-independent and still work.
        kb.onKey(Key.Shift)
        assertTrue(kb.isShifted)
        kb.onKey(Key.SwitchToVoice)
        assertEquals(1, switched)
    }
}
