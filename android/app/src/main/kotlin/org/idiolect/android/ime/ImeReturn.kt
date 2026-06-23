package org.idiolect.android.ime

/**
 * The pure rule for the auto-return to idiolect's mic after a reviewed Insert: whether the
 * default IME needs rewriting back to idiolect.
 *
 * The *timing* is the load-bearing part and lives in
 * [org.idiolect.android.accessibility.IdiolectAccessibilityService], not here: the rewrite must
 * happen once the review dialog is gone and the host field has regained focus (a focus event),
 * NOT from the dialog before it finishes. Writing it while the review field is still focused
 * makes idiolect re-bind that field, hand back off to the user's keyboard, and the default
 * bounces straight back — so the mic never returns. That lifecycle has no headless seam (it's
 * covered by the connected e2e); this decision is the unit-tested part.
 */
object ImeReturn {
    /** Whether to rewrite the default IME back to idiolect — only when it isn't already. */
    fun shouldRestore(currentDefaultIme: String?, idiolectIme: String): Boolean =
        currentDefaultIme != idiolectIme
}
