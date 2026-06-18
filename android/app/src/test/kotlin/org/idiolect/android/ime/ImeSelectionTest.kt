package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Unit cover for the IME-id the review surface writes to `Settings.Secure.DEFAULT_INPUT_METHOD`
 * to pull the active keyboard back to idiolect after Insert (the auto-return the user asked for).
 *
 * The id MUST be the *fully-qualified* flattened component — `pkg/full.Class` — exactly as the
 * framework stores it. The classic bug is the abbreviated `pkg/.Class` short form, which never
 * matches the stored value and so silently fails to switch; this guards against that.
 */
class ImeSelectionTest {
    @Test
    fun builds_the_fully_qualified_flattened_component() {
        assertEquals(
            "org.idiolect.android/org.idiolect.android.ime.IdiolectImeService",
            ImeSelection.idiolectImeId(
                packageName = "org.idiolect.android",
                serviceClass = "org.idiolect.android.ime.IdiolectImeService",
            ),
        )
    }

    @Test
    fun does_not_abbreviate_a_class_in_the_package() {
        // Even though the class shares the package prefix, the stored value is NOT shortened.
        val id = ImeSelection.idiolectImeId("com.x", "com.x.Ime")
        assertEquals("com.x/com.x.Ime", id)
    }
}
