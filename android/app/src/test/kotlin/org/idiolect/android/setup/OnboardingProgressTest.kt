package org.idiolect.android.setup

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The step-indicator maths for the onboarding screen: how many of the four setup gates
 * (enable → select → microphone → model) are already cleared at a given [ImeSetupStep]. Pure,
 * so the "● ● ○ ○ — step 3 of 4" dots are unit-tested and the activity just renders the count.
 */
class OnboardingProgressTest {
    @Test
    fun the_first_gate_has_nothing_done_yet() {
        assertEquals(0 to 4, OnboardingProgress.of(ImeSetupStep.EnableKeyboard))
    }

    @Test
    fun each_gate_advances_the_completed_count() {
        assertEquals(1 to 4, OnboardingProgress.of(ImeSetupStep.SelectKeyboard))
        assertEquals(2 to 4, OnboardingProgress.of(ImeSetupStep.GrantMicrophone))
        assertEquals(3 to 4, OnboardingProgress.of(ImeSetupStep.DownloadModel))
    }

    @Test
    fun ready_means_all_four_gates_are_cleared() {
        assertEquals(4 to 4, OnboardingProgress.of(ImeSetupStep.Ready))
    }
}
