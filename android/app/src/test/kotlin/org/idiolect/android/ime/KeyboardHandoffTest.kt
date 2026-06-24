package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * idiolect never renders its own typing keyboard — to edit, it hands the field to the
 * user's **own** keyboard via the system IME switch. [SwitchToYourKeyboard] picks the
 * fallback order (last-used → next → the system picker) so the real `InputMethodService`
 * calls stay a thin, untestable boundary; [KeyboardTargets] picks which enabled keyboard
 * to switch to when there's no switch history.
 */
class KeyboardHandoffTest {
    /** Records which switch primitives were tried, and in what order. */
    private class FakeHandoff(
        private val previousWorks: Boolean = true,
        private val nextWorks: Boolean = true,
    ) : KeyboardHandoff {
        val tried = mutableListOf<String>()
        override fun toPreviousKeyboard(): Boolean {
            tried.add("previous"); return previousWorks
        }
        override fun toNextKeyboard(): Boolean {
            tried.add("next"); return nextWorks
        }
        override fun openPicker() {
            tried.add("picker")
        }
    }

    @Test
    fun the_last_used_keyboard_is_preferred() {
        val handoff = FakeHandoff(previousWorks = true)
        SwitchToYourKeyboard.run(handoff)
        // The user's previous keyboard took it — don't cycle further or pop a picker.
        assertEquals(listOf("previous"), handoff.tried)
    }

    @Test
    fun falls_through_to_the_next_keyboard_when_there_is_no_previous() {
        val handoff = FakeHandoff(previousWorks = false, nextWorks = true)
        SwitchToYourKeyboard.run(handoff)
        assertEquals(listOf("previous", "next"), handoff.tried)
    }

    @Test
    fun opens_the_picker_only_as_a_last_resort() {
        val handoff = FakeHandoff(previousWorks = false, nextWorks = false)
        SwitchToYourKeyboard.run(handoff)
        assertEquals(listOf("previous", "next", "picker"), handoff.tried)
    }

    // --- Target picking: when there's no switch history, hand off to a specific other
    // enabled keyboard by id (the deterministic path, since the system picker is unreliable
    // from an IME). ---

    @Test
    fun picks_the_first_enabled_keyboard_that_isnt_ours() {
        val enabled = listOf(
            EnabledKeyboard("org.idiolect.android/.ime.IdiolectImeService", "org.idiolect.android"),
            EnabledKeyboard("helium314.keyboard/.latin.LatinIME", "helium314.keyboard"),
            EnabledKeyboard("com.google.android.inputmethod.latin/.LatinIME", "com.google.android.inputmethod.latin"),
        )
        assertEquals(
            "helium314.keyboard/.latin.LatinIME",
            KeyboardTargets.pickOther(enabled, ownPackage = "org.idiolect.android"),
        )
    }

    @Test
    fun returns_null_when_idiolect_is_the_only_enabled_keyboard() {
        val enabled = listOf(
            EnabledKeyboard("org.idiolect.android/.ime.IdiolectImeService", "org.idiolect.android"),
        )
        assertNull(KeyboardTargets.pickOther(enabled, ownPackage = "org.idiolect.android"))
    }
}
