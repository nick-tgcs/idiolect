package org.idiolect.android.ime

import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Unit cover for [EnterAction.forEditor] — the pure decision behind the ⏎ enter key on the
 * mic surface (Option A). A field that declares a concrete IME action wants that action
 * performed; anything else gets a literal newline. The framework `EditorInfo` constants are
 * compile-time `int`s (inlined), so this needs no Android runtime.
 */
class EnterActionTest {
    @Test
    fun a_search_field_performs_the_search_action() {
        assertEquals(
            EnterAction.Editor(EditorInfo.IME_ACTION_SEARCH),
            EnterAction.forEditor(EditorInfo.IME_ACTION_SEARCH),
        )
    }

    @Test
    fun a_send_field_performs_the_send_action() {
        assertEquals(
            EnterAction.Editor(EditorInfo.IME_ACTION_SEND),
            EnterAction.forEditor(EditorInfo.IME_ACTION_SEND),
        )
    }

    @Test
    fun the_action_is_extracted_from_the_masked_bits_even_with_other_flags_set() {
        // Real fields OR extra flags into imeOptions; only the low IME_MASK_ACTION bits are the action.
        val opts = EditorInfo.IME_ACTION_DONE or EditorInfo.IME_FLAG_NO_FULLSCREEN
        assertEquals(EnterAction.Editor(EditorInfo.IME_ACTION_DONE), EnterAction.forEditor(opts))
    }

    @Test
    fun an_unspecified_action_inserts_a_newline() {
        assertEquals(EnterAction.Newline, EnterAction.forEditor(EditorInfo.IME_ACTION_UNSPECIFIED))
    }

    @Test
    fun an_explicit_none_action_inserts_a_newline() {
        assertEquals(EnterAction.Newline, EnterAction.forEditor(EditorInfo.IME_ACTION_NONE))
    }

    @Test
    fun a_field_that_opts_out_with_no_enter_action_inserts_a_newline() {
        // IME_FLAG_NO_ENTER_ACTION means "don't treat enter as the action" even for a multiline
        // field that carries one — the user wants a newline.
        val opts = EditorInfo.IME_ACTION_SEARCH or EditorInfo.IME_FLAG_NO_ENTER_ACTION
        assertEquals(EnterAction.Newline, EnterAction.forEditor(opts))
    }
}
