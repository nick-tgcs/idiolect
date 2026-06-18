package org.idiolect.android.accessibility

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit cover for [InjectionTargeting.isHostField] — which focused field the accessibility
 * service should remember as the injection target. It must be an editable field in some
 * *other* app's window (the field the user dictated into), never idiolect's own review
 * dialog (our package) — otherwise Insert would type the correction back into the card it
 * came from. The framework event plumbing that feeds this is e2e/manual; the rule is pure.
 */
class InjectionTargetingTest {
    @Test
    fun an_editable_field_in_another_app_is_the_target() {
        assertTrue(InjectionTargeting.isHostField("com.some.app", ownPackage = OWN, isEditable = true))
    }

    @Test
    fun our_own_review_dialog_field_is_never_the_target() {
        assertFalse(InjectionTargeting.isHostField(OWN, ownPackage = OWN, isEditable = true))
    }

    @Test
    fun a_non_editable_view_is_not_a_target() {
        assertFalse(InjectionTargeting.isHostField("com.some.app", ownPackage = OWN, isEditable = false))
    }

    @Test
    fun a_missing_package_is_not_a_target() {
        assertFalse(InjectionTargeting.isHostField(null, ownPackage = OWN, isEditable = true))
    }

    private companion object {
        const val OWN = "org.idiolect.android"
    }
}
