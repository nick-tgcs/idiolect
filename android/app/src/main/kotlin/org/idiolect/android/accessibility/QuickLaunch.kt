package org.idiolect.android.accessibility

/** What a tap on Android's floating accessibility ("quick-launch") button should do. */
enum class QuickLaunchAction {
    /** The in-app toggle is off — don't record; nudge the user to Settings instead. */
    Disabled,

    /** Idle: open the mic and dictate into the focused field. */
    Start,

    /** A take is already running: finalize it. */
    Stop,
}

/**
 * The quick-launch button policy, kept pure so tap-to-start / tap-to-stop and the toggle override
 * are unit-tested away from [IdiolectAccessibilityService]'s un-headless button + capture wiring.
 */
object QuickLaunch {
    fun decide(enabled: Boolean, recording: Boolean): QuickLaunchAction = when {
        !enabled -> QuickLaunchAction.Disabled
        recording -> QuickLaunchAction.Stop
        else -> QuickLaunchAction.Start
    }
}
