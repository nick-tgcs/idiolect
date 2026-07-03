package org.idiolect.android.ime

import android.view.inputmethod.EditorInfo
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The ⏎ key mirrors the system Enter for the focused field: perform its declared
 * editor action, or insert a newline when it declares none / suppresses the action.
 * Pure bitfield logic, host-JVM-testable.
 */
class EnterKeyActionTest {
    @Test
    fun a_declared_action_is_performed() {
        assertEquals(
            EnterKeyAction.Decision.PerformAction(EditorInfo.IME_ACTION_SEND),
            EnterKeyAction.decide(EditorInfo.IME_ACTION_SEND),
        )
        assertEquals(
            EnterKeyAction.Decision.PerformAction(EditorInfo.IME_ACTION_SEARCH),
            EnterKeyAction.decide(EditorInfo.IME_ACTION_SEARCH),
        )
    }

    @Test
    fun no_action_or_unspecified_falls_back_to_a_newline() {
        assertEquals(EnterKeyAction.Decision.InsertNewline, EnterKeyAction.decide(EditorInfo.IME_ACTION_NONE))
        assertEquals(
            EnterKeyAction.Decision.InsertNewline,
            EnterKeyAction.decide(EditorInfo.IME_ACTION_UNSPECIFIED),
        )
        // A multi-line field typically leaves the action unspecified.
        assertEquals(EnterKeyAction.Decision.InsertNewline, EnterKeyAction.decide(0))
    }

    @Test
    fun the_no_enter_action_flag_forces_a_newline_even_with_an_action() {
        // A field can declare an action for its own IME button yet ask the Enter key to
        // stay a newline (IME_FLAG_NO_ENTER_ACTION) — respect that.
        val options = EditorInfo.IME_ACTION_GO or EditorInfo.IME_FLAG_NO_ENTER_ACTION
        assertEquals(EnterKeyAction.Decision.InsertNewline, EnterKeyAction.decide(options))
    }
}
