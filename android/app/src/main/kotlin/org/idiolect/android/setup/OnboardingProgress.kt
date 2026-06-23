package org.idiolect.android.setup

/**
 * The step-indicator maths for the onboarding screen. The four setup gates are, in order,
 * enable → select → grant the mic → download the model; [OnboardingProgress.of] returns how
 * many are already cleared at a given [ImeSetupStep] (and the total), so the screen can draw a
 * "● ● ○ ○ — step 3 of 4" indicator without duplicating the step logic. Pure and unit-tested.
 */
object OnboardingProgress {
    const val TOTAL_GATES = 4

    /** (gates cleared, total) at [step]. EnableKeyboard = 0 cleared; Ready = all four. */
    fun of(step: ImeSetupStep): Pair<Int, Int> = when (step) {
        ImeSetupStep.EnableKeyboard -> 0 to TOTAL_GATES
        ImeSetupStep.SelectKeyboard -> 1 to TOTAL_GATES
        ImeSetupStep.GrantMicrophone -> 2 to TOTAL_GATES
        ImeSetupStep.DownloadModel -> 3 to TOTAL_GATES
        ImeSetupStep.Ready -> TOTAL_GATES to TOTAL_GATES
    }
}
