package org.idiolect.android.setup

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The onboarding step machine: given what's already configured, what is the single
 * next thing the user must do? The keyboard has to be enabled and selected before it
 * can be used at all; the microphone is the last gate before dictation works.
 */
class ImeSetupTest {
    @Test
    fun a_disabled_keyboard_is_enabled_first_regardless_of_other_state() {
        assertEquals(
            ImeSetupStep.EnableKeyboard,
            ImeSetup.nextStep(hasMicPermission = false, isEnabled = false, isSelected = false),
        )
        assertEquals(
            ImeSetupStep.EnableKeyboard,
            ImeSetup.nextStep(hasMicPermission = true, isEnabled = false, isSelected = true),
        )
    }

    @Test
    fun an_enabled_but_unselected_keyboard_is_selected_next() {
        assertEquals(
            ImeSetupStep.SelectKeyboard,
            ImeSetup.nextStep(hasMicPermission = true, isEnabled = true, isSelected = false),
        )
    }

    @Test
    fun a_selected_keyboard_without_mic_permission_asks_for_the_microphone() {
        assertEquals(
            ImeSetupStep.GrantMicrophone,
            ImeSetup.nextStep(hasMicPermission = false, isEnabled = true, isSelected = true),
        )
    }

    @Test
    fun everything_satisfied_is_ready() {
        assertEquals(
            ImeSetupStep.Ready,
            ImeSetup.nextStep(hasMicPermission = true, isEnabled = true, isSelected = true),
        )
    }
}
