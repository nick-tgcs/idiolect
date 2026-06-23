package org.idiolect.android.settings

import org.idiolect.android.model.InstalledModel
import org.idiolect.android.sync.SyncSettings

/**
 * A snapshot of the persisted toggles ([SettingsStore]), passed by value so the presenter is
 * pure (no file I/O) and the activity reads the store once per render.
 */
data class PrefsSnapshot(
    val reviewByDefault: Boolean,
    val continuousOnDoubleTap: Boolean,
    val shipCorrections: Boolean,
)

/** Whether the keyboard is usable, read from the framework by the activity glue. */
data class SystemStatus(
    val keyboardEnabled: Boolean,
    val keyboardSelected: Boolean,
    val micGranted: Boolean,
)

/** How the "Connect to PC" hero renders. */
sealed interface ConnectionView {
    /** No endpoint configured — show the scan-to-pair call to action. */
    data object Unpaired : ConnectionView

    /** A paired endpoint, with its pin state shown for verification / honesty. */
    data class Paired(val endpoint: String, val pin: PinView) : ConnectionView
}

/** The trust state of a paired endpoint. */
sealed interface PinView {
    /** TLS (the default): the SPKI pin, grouped into quads for human comparison. */
    data class Pinned(val fingerprintGrouped: String) : PinView

    /** The `--no-tls` cleartext fallback — there is no cert to pin, and the card says so. */
    data object Cleartext : PinView
}

/** The whole settings screen, decided purely from device inputs. */
data class SettingsViewState(
    val connection: ConnectionView,
    val modelLabel: String,
    val reviewOn: Boolean,
    val continuousOn: Boolean,
    val shipOn: Boolean,
    val audioLabel: String,
    val system: SystemStatus,
)

/** Builds a [SettingsViewState] from the raw device state. The screen's only logic lives here. */
object SettingsView {
    fun from(
        paired: SyncSettings?,
        model: InstalledModel?,
        prefs: PrefsSnapshot,
        system: SystemStatus,
        audioUsedBytes: Long,
        audioCapBytes: Long,
    ): SettingsViewState = SettingsViewState(
        connection = connectionOf(paired),
        modelLabel = model?.let { "${it.id} · on-device" } ?: "No model yet",
        reviewOn = prefs.reviewByDefault,
        continuousOn = prefs.continuousOnDoubleTap,
        shipOn = prefs.shipCorrections,
        audioLabel = AudioUsage.format(audioUsedBytes, audioCapBytes),
        system = system,
    )

    private fun connectionOf(paired: SyncSettings?): ConnectionView {
        if (paired == null) return ConnectionView.Unpaired
        // A pin is present iff the endpoint is the pinned-TLS default (the transport refuses an
        // unpinned https); a cleartext --no-tls endpoint pairs with no pin.
        val pin = paired.pin?.let { PinView.Pinned(CertFingerprint.grouped(it)) } ?: PinView.Cleartext
        return ConnectionView.Paired(paired.baseUrl, pin)
    }
}
