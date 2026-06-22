package org.idiolect.android.setup

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Where an `idiolect://pair` deep link is handled. The link is the same one the PC's `--pair`
 * QR encodes; the only question is *which screen* acts on it:
 *
 *  - **Onboarding** for a device that isn't paired yet — first enrolment goes through
 *    [SetupActivity], which also pulls the model from the now-paired PC.
 *  - **Settings** for an already-paired device — a re-pair is a lean swap of endpoint/token/pin
 *    shown in context on the ⚙ screen, with no redundant model re-download.
 *  - **Ignore** for any non-pairing launch (the normal MAIN/LAUNCHER case, or another URL).
 *
 * A pure decision so the routing is unit-tested rather than buried in activity glue (the
 * [ImeSetup] / [ModelSourceChoice] pattern).
 */
class PairingRouterTest {
    @Test
    fun an_unpaired_device_pairs_through_onboarding() {
        assertEquals(
            PairingLinkRoute.Onboarding,
            PairingRouter.route(isPairingLink = true, alreadyPaired = false),
        )
    }

    @Test
    fun an_already_paired_device_re_pairs_in_settings() {
        assertEquals(
            PairingLinkRoute.Settings,
            PairingRouter.route(isPairingLink = true, alreadyPaired = true),
        )
    }

    @Test
    fun a_non_pairing_launch_is_ignored_regardless_of_pairing_state() {
        assertEquals(
            PairingLinkRoute.Ignore,
            PairingRouter.route(isPairingLink = false, alreadyPaired = false),
        )
        assertEquals(
            PairingLinkRoute.Ignore,
            PairingRouter.route(isPairingLink = false, alreadyPaired = true),
        )
    }
}
