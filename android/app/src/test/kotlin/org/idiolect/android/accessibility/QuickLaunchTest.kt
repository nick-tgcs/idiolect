package org.idiolect.android.accessibility

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * What a tap on Android's floating accessibility ("quick-launch") button does, decided purely so
 * the policy is pinned away from the un-headless service wiring: the in-app toggle wins (an
 * off feature never records), and while on, a tap starts a take when idle and stops it when one
 * is running (tap-to-start, tap-to-stop). The take + the inject-into-focused-field that follow
 * are covered by the connected e2e.
 */
class QuickLaunchTest {
    @Test
    fun the_off_toggle_wins_regardless_of_state() {
        assertEquals(QuickLaunchAction.Disabled, QuickLaunch.decide(enabled = false, recording = false))
        assertEquals(QuickLaunchAction.Disabled, QuickLaunch.decide(enabled = false, recording = true))
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
