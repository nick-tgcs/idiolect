package org.idiolect.android.accessibility

/**
 * The pure rule for which focused field the [IdiolectAccessibilityService] remembers as the
 * injection target: an editable field in *another* app (the field the user dictated into),
 * never idiolect's own review dialog. Separated from the event plumbing so the rule is
 * unit-tested; the [android.view.accessibility.AccessibilityEvent] feed is e2e/manual.
 */
object InjectionTargeting {
    fun isHostField(eventPackage: String?, ownPackage: String, isEditable: Boolean): Boolean =
        isEditable && eventPackage != null && eventPackage != ownPackage
}

/**
 * Whether idiolect's "instant insert" accessibility service is currently enabled, parsed from
 * the system's `enabled_accessibility_services` setting. The review dialog uses this to decide
 * between injecting (service on) and deferring to the IME (service off), and whether to show
 * the one-time "enable" nudge — a cross-process check, so it works from the dialog regardless
 * of which process the service is bound in.
 */
object AccessibilityServices {
    fun isListed(enabledSetting: String?, component: String): Boolean =
        enabledSetting?.split(':')?.any { it == component } == true
}
