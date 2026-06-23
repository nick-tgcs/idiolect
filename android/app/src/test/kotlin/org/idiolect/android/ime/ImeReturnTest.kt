package org.idiolect.android.ime

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit cover for [ImeReturn.shouldRestore] — the pure guard behind the auto-return to idiolect's
 * mic after a reviewed Insert. The default IME is only rewritten back to idiolect when something
 * else is currently the default; rewriting it when idiolect is already active is a needless
 * secure-settings write (and an avoidable keyboard re-bind). The *when* — once the review dialog
 * is gone and the host field has focus, never from the dialog (which bounces) — is the
 * connected-e2e's job; this is the unit-tested decision.
 */
class ImeReturnTest {
    private val idiolect = "org.idiolect.android/org.idiolect.android.ime.IdiolectImeService"

    @Test
    fun restores_when_another_keyboard_is_active() {
        val latin = "com.google.android.inputmethod.latin/com.android.inputmethod.latin.LatinIME"
        assertTrue(ImeReturn.shouldRestore(latin, idiolect))
    }

    @Test
    fun restores_when_no_default_is_set() {
        assertTrue(ImeReturn.shouldRestore(null, idiolect))
    }

    @Test
    fun does_not_rewrite_when_idiolect_is_already_active() {
        assertFalse(ImeReturn.shouldRestore(idiolect, idiolect))
    }
}
