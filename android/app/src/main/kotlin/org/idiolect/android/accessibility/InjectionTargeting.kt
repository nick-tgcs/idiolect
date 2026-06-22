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
    fun isListed(enabledSetting: String?, component: String): Boolean {
        val target = canonical(component)
        return enabledSetting?.split(':')?.any { canonical(it) == target } == true
    }

    /**
     * Resolve a flattened `pkg/class` component to its long form. Android stores enabled
     * services with [android.content.ComponentName.flattenToShortString], which abbreviates a
     * class under its own package to a leading dot (`pkg/.Sub.Class`); the dialog asks with the
     * long form (`pkg/pkg.Sub.Class`). Expanding the leading dot makes both compare equal, so a
     * service enabled via system settings is recognised. Entries without a `/` pass through
     * unchanged (never equal to a real component).
     */
    private fun canonical(flattened: String): String {
        val slash = flattened.indexOf('/')
        if (slash < 0) return flattened
        val pkg = flattened.substring(0, slash)
        val cls = flattened.substring(slash + 1)
        val fullClass = if (cls.startsWith(".")) pkg + cls else cls
        return "$pkg/$fullClass"
    }
}
