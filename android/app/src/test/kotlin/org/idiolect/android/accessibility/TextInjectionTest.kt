package org.idiolect.android.accessibility

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Unit cover for [TextInjection.spliceAtSelection] — the pure decision of WHAT a field's text
 * and cursor become when the reviewed (corrected) text is dropped in at the cursor/selection.
 * The accessibility node manipulation that applies it is framework glue (no headless seam),
 * exercised by the connected e2e; this nails the splice arithmetic deterministically.
 */
class TextInjectionTest {
    @Test
    fun inserts_at_a_collapsed_cursor_in_the_middle() {
        val r = TextInjection.spliceAtSelection("ab", 1, 1, "XYZ")
        assertEquals("aXYZb", r.text)
        assertEquals(4, r.cursor) // right after the inserted run
    }

    @Test
    fun replaces_a_selected_range() {
        val r = TextInjection.spliceAtSelection("hello world", 6, 11, "there")
        assertEquals("hello there", r.text)
        assertEquals(11, r.cursor)
    }

    @Test
    fun normalises_a_backwards_selection() {
        // selStart > selEnd (the user dragged right-to-left) must behave the same.
        val r = TextInjection.spliceAtSelection("hello world", 11, 6, "there")
        assertEquals("hello there", r.text)
        assertEquals(11, r.cursor)
    }

    @Test
    fun appends_at_the_end_when_there_is_no_selection() {
        // A node with no selection reports -1/-1; treat that as "type at the end".
        val r = TextInjection.spliceAtSelection("note: ", -1, -1, "done")
        assertEquals("note: done", r.text)
        assertEquals(10, r.cursor)
    }

    @Test
    fun into_an_empty_field_just_becomes_the_text() {
        val r = TextInjection.spliceAtSelection("", 0, 0, "fresh")
        assertEquals("fresh", r.text)
        assertEquals(5, r.cursor)
    }

    @Test
    fun clamps_an_out_of_range_selection_to_the_text() {
        // Stale selection indices past the end must not throw — clamp to the field length.
        val r = TextInjection.spliceAtSelection("ab", 5, 9, "Z")
        assertEquals("abZ", r.text)
        assertEquals(3, r.cursor)
    }
}
