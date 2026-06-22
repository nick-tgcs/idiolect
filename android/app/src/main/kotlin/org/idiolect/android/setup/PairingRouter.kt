package org.idiolect.android.setup

/** Which screen acts on an `idiolect://pair` deep link. */
enum class PairingLinkRoute {
    /** Not a pairing link — leave the launch alone. */
    Ignore,

    /** First enrolment: pair through onboarding ([SetupActivity]), which also pulls the model. */
    Onboarding,

    /** Re-pair on an already-paired device: a lean endpoint/token/pin swap shown on the ⚙ screen. */
    Settings,
}

/**
 * Decides where an `idiolect://pair` deep link is handled — the same link the PC's `--pair` QR
 * encodes. The choice is purely (is it a pairing link?) × (is the device already paired?), so it
 * lives here, unit-tested, rather than in [SetupActivity] glue. [SetupActivity] is the manifest's
 * sole deep-link target; when this says [PairingLinkRoute.Settings] it forwards the link to
 * [org.idiolect.android.settings.SettingsActivity] instead of re-running onboarding.
 */
object PairingRouter {
    fun route(isPairingLink: Boolean, alreadyPaired: Boolean): PairingLinkRoute = when {
        !isPairingLink -> PairingLinkRoute.Ignore
        alreadyPaired -> PairingLinkRoute.Settings
        else -> PairingLinkRoute.Onboarding
    }
}
