package org.idiolect.android.accessibility

/** What a tap on Android's floating accessibility ("quick-launch") button should do. */
enum class QuickLaunchAction {
    /** The in-app toggle is off and nothing records — don't start; nudge the user to Settings. */
    Disabled,

    /** Idle: open the mic and dictate into the focused field. */
    Start,

    /** A take is already running: end it. Finalizing (not discarding) even if the toggle was
     *  just switched off — the speech was dictated while enabled, and it still lands. */
    Stop,
}

/**
 * The quick-launch button policy, kept pure so tap-to-start / tap-to-stop and the toggle override
 * are unit-tested away from [IdiolectAccessibilityService]'s un-headless button + capture wiring.
 * The toggle gates only *starting* (mirroring [org.idiolect.android.ime.MicToggle]'s canStart):
 * a live take must always be stoppable, or switching the feature off mid-take leaves a hot mic
 * whose own button answers "enable in settings".
 */
object QuickLaunch {
    fun decide(enabled: Boolean, recording: Boolean): QuickLaunchAction = when {
        recording -> QuickLaunchAction.Stop
        !enabled -> QuickLaunchAction.Disabled
        else -> QuickLaunchAction.Start
    }
}
