package org.idiolect.android.accessibility

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * What a tap on Android's floating accessibility ("quick-launch") button does, decided purely so
 * the policy is pinned away from the un-headless service wiring: the in-app toggle blocks
 * *starting* (an off feature never begins recording), and a tap starts a take when idle and
 * stops it when one is running (tap-to-start, tap-to-stop). The take + the
 * inject-into-focused-field that follow are covered by the connected e2e.
 */
class QuickLaunchTest {
    @Test
    fun the_off_toggle_blocks_starting_a_take() {
        assertEquals(QuickLaunchAction.Disabled, QuickLaunch.decide(enabled = false, recording = false))
    }

    @Test
    fun a_live_take_is_always_stoppable_even_after_the_toggle_is_turned_off() {
        // The toggle gates only starting (mirrors MicToggle's canStart). If Disabled won here,
        // switching the feature off mid-take would leave the mic recording with the very button
        // that started it answering "enable in settings" — a hot mic with no stop.
        assertEquals(QuickLaunchAction.Stop, QuickLaunch.decide(enabled = false, recording = true))
    }

    @Test
    fun a_tap_while_idle_starts_a_take() {
        assertEquals(QuickLaunchAction.Start, QuickLaunch.decide(enabled = true, recording = false))
    }

    @Test
    fun a_tap_while_recording_stops_the_take() {
        assertEquals(QuickLaunchAction.Stop, QuickLaunch.decide(enabled = true, recording = true))
    }
}
