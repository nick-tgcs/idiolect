package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The IME surface is one view with two modes ([KeyboardMode.Voice] / [KeyboardMode.Edit])
 * that swap in place; the toggle is symmetric and always one tap from either mode (the
 * product's hard requirement — see plan §1.3).
 */
class ModePresenterTest {
    @Test
    fun voice_is_the_default() {
        assertEquals(KeyboardMode.Voice, ModePresenter().current())
    }

    @Test
    fun toggle_flips_voice_to_edit_and_back() {
        val presenter = ModePresenter()
        assertEquals(KeyboardMode.Edit, presenter.toggle())
        assertEquals(KeyboardMode.Edit, presenter.current())
        assertEquals(KeyboardMode.Voice, presenter.toggle())
        assertEquals(KeyboardMode.Voice, presenter.current())
    }

    @Test
    fun show_forces_a_specific_mode() {
        val presenter = ModePresenter()
        assertEquals(KeyboardMode.Edit, presenter.show(KeyboardMode.Edit))
        assertEquals(KeyboardMode.Edit, presenter.current())
        // Idempotent — re-showing the active mode keeps it.
        assertEquals(KeyboardMode.Edit, presenter.show(KeyboardMode.Edit))
    }
}
