package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * Unit cover for the IME-id the review surface writes to `Settings.Secure.DEFAULT_INPUT_METHOD`
 * to pull the active keyboard back to idiolect after Insert (the auto-return the user asked for).
 *
 * The id must be the framework's OWN string for idiolect's IME — its `flattenToShortString`
 * form, abbreviated to a leading dot (`pkg/.ime.Class`). The classic bug is reconstructing the
 * fully-qualified long form `pkg/pkg.ime.Class`, which InputMethodManagerService rejects as
 * "Unknown id" so the switch silently fails. Picking the id from the enabled list avoids that.
 */
class ImeSelectionTest {
    private val own = "org.idiolect.android"

    @Test
    fun picks_idiolects_framework_id_verbatim() {
        // The framework registers idiolect's IME by its short-form id (leading dot) — hand THAT
        // back, not a reconstructed long form.
        val enabled = listOf(
            EnabledKeyboard(
                "com.google.android.inputmethod.latin/com.android.inputmethod.latin.LatinIME",
                "com.google.android.inputmethod.latin",
            ),
            EnabledKeyboard("org.idiolect.android/.ime.IdiolectImeService", own),
        )
        assertEquals(
            "org.idiolect.android/.ime.IdiolectImeService",
            ImeSelection.idiolectImeId(enabled, own),
        )
    }

    @Test
    fun null_when_idiolect_is_not_enabled() {
        val enabled = listOf(EnabledKeyboard("com.x/.Ime", "com.x"))
        assertNull(ImeSelection.idiolectImeId(enabled, own))
    }

    @Test
    fun null_for_an_empty_list() {
        assertNull(ImeSelection.idiolectImeId(emptyList(), own))
    }
}
