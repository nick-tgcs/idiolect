package org.idiolect.android.accessibility

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Unit cover for [AccessibilityServices.isListed] — the pure parse of the system's
 * colon-separated `enabled_accessibility_services` setting, used to decide whether instant
 * insert is on (so the review dialog injects vs. defers, and shows/hides the "enable" nudge).
 */
class AccessibilityServicesTest {
    private val component = "org.idiolect.android/org.idiolect.android.accessibility.IdiolectAccessibilityService"

    @Test
    fun finds_our_component_among_others() {
        assertTrue(AccessibilityServices.isListed("com.other/.Svc:$component:com.x/.Y", component))
    }

    @Test
    fun finds_our_component_alone() {
        assertTrue(AccessibilityServices.isListed(component, component))
    }

    @Test
    fun absent_when_not_listed() {
        assertFalse(AccessibilityServices.isListed("com.other/.Svc", component))
    }

    @Test
    fun absent_for_null_or_empty() {
        assertFalse(AccessibilityServices.isListed(null, component))
        assertFalse(AccessibilityServices.isListed("", component))
    }

    @Test
    fun a_substring_match_does_not_count() {
        // A different service whose name merely contains ours must not register as enabled.
        assertFalse(AccessibilityServices.isListed("${component}Extra", component))
    }

    @Test
    fun matches_the_systems_short_form_with_a_leading_dot() {
        // Android stores enabled services in flattenToShortString form: when the class is under
        // the package it abbreviates to "pkg/.Sub.Class". The dialog asks with the long form, so
        // a raw string compare misses it and wrongly shows the "enable" nudge. They must match.
        val shortForm = "org.idiolect.android/.accessibility.IdiolectAccessibilityService"
        assertTrue(AccessibilityServices.isListed(shortForm, component))
        assertTrue(AccessibilityServices.isListed("com.other/.Svc:$shortForm:com.x/.Y", component))
    }

    @Test
    fun a_short_form_for_a_different_class_does_not_count() {
        // Normalisation must not over-match: a sibling service in the same package is not ours.
        val other = "org.idiolect.android/.accessibility.SomethingElse"
        assertFalse(AccessibilityServices.isListed(other, component))
    }
}
