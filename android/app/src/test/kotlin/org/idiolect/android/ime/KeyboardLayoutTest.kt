package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The v1 tap-only QWERTY layout (no autocorrect/prediction — deliberately small and
 * predictable; plan §1.3). The layout is pure data so it is host-testable; the actual
 * key rendering is the declared GUI seam.
 */
class KeyboardLayoutTest {
    private fun letters(row: List<Key>): String =
        row.filterIsInstance<Key.Character>().joinToString("") { it.lower }

    @Test
    fun has_the_three_letter_rows_in_qwerty_order() {
        val rows = KeyboardLayout.QWERTY
        assertEquals("qwertyuiop", letters(rows[0]))
        assertEquals("asdfghjkl", letters(rows[1]))
        assertEquals("zxcvbnm", letters(rows[2]))
    }

    @Test
    fun the_letter_row_is_flanked_by_shift_and_backspace() {
        val row = KeyboardLayout.QWERTY[2]
        assertEquals(Key.Shift, row.first())
        assertEquals(Key.Backspace, row.last())
    }

    @Test
    fun the_bottom_row_returns_to_voice_and_has_space_and_enter() {
        val bottom = KeyboardLayout.QWERTY.last()
        assertTrue(Key.SwitchToVoice in bottom)
        assertTrue(Key.Space in bottom)
        assertTrue(Key.Enter in bottom)
    }

    @Test
    fun character_keys_carry_matching_upper_case() {
        val chars = KeyboardLayout.QWERTY.flatten().filterIsInstance<Key.Character>()
        assertTrue(chars.isNotEmpty())
        chars.forEach { assertEquals(it.lower.uppercase(), it.upper) }
    }
}
