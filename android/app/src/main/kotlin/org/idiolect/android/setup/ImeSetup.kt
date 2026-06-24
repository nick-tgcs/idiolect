package org.idiolect.android.setup

/** The single next onboarding action, or [Ready] when dictation is usable. */
enum class ImeSetupStep {
    EnableKeyboard,
    SelectKeyboard,
    GrantMicrophone,
    DownloadModel,
    Ready,
}

/** Pure onboarding logic, separated from the framework calls that read the state. */
object ImeSetup {
    /**
     * The next step to surface. Enable then select the keyboard (it cannot be used
     * otherwise), then grant the microphone, then download a speech model — the last
     * gate before dictation actually produces text.
     */
    fun nextStep(
        hasMicPermission: Boolean,
        isEnabled: Boolean,
        isSelected: Boolean,
        hasModel: Boolean,
    ): ImeSetupStep = when {
        !isEnabled -> ImeSetupStep.EnableKeyboard
        !isSelected -> ImeSetupStep.SelectKeyboard
        !hasMicPermission -> ImeSetupStep.GrantMicrophone
        !hasModel -> ImeSetupStep.DownloadModel
        else -> ImeSetupStep.Ready
    }
}
