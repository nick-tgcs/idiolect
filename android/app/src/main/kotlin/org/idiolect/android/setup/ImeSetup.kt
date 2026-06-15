package org.idiolect.android.setup

/** The single next onboarding action, or [Ready] when dictation is usable. */
enum class ImeSetupStep {
    EnableKeyboard,
    SelectKeyboard,
    GrantMicrophone,
    Ready,
}

/** Pure onboarding logic, separated from the framework calls that read the state. */
object ImeSetup {
    /**
     * The next step to surface. Enable then select the keyboard (it cannot be used
     * otherwise), then grant the microphone (the last gate before dictation works).
     */
    fun nextStep(
        hasMicPermission: Boolean,
        isEnabled: Boolean,
        isSelected: Boolean,
    ): ImeSetupStep = when {
        !isEnabled -> ImeSetupStep.EnableKeyboard
        !isSelected -> ImeSetupStep.SelectKeyboard
        !hasMicPermission -> ImeSetupStep.GrantMicrophone
        else -> ImeSetupStep.Ready
    }
}
