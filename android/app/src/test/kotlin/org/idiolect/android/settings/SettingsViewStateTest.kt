package org.idiolect.android.settings

import org.idiolect.android.model.InstalledModel
import org.idiolect.android.sync.SyncSettings
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The pure render model for the settings screen: every section the activity draws is decided
 * here from device inputs (paired endpoint, installed model, persisted toggles, system status,
 * audio footprint), so the activity stays dumb glue and the *decisions* — paired vs unpaired,
 * pinned vs cleartext, model present vs absent — are unit-tested. Mirrors `ImeSetup.nextStep`
 * and `VoiceModePresenter`: no Android types, total, exhaustively covered.
 */
class SettingsViewStateTest {
    private val realPin =
        "94e32367cdca173650e3dbd73f4a2dc657e0951c9cc0974556a6909fe5216c3c"

    private fun state(
        paired: SyncSettings? = null,
        model: InstalledModel? = null,
        prefs: PrefsSnapshot = PrefsSnapshot(
            reviewByDefault = false,
            continuousOnDoubleTap = true,
            shipCorrections = true,
            quickLaunchMic = true,
        ),
        system: SystemStatus = SystemStatus(keyboardEnabled = true, keyboardSelected = true, micGranted = true),
        audioUsedBytes: Long = 0,
        audioCapBytes: Long = 1024L * 1024 * 1024,
    ) = SettingsView.from(paired, model, prefs, system, audioUsedBytes, audioCapBytes)

    @Test
    fun no_endpoint_renders_as_unpaired() {
        assertEquals(ConnectionView.Unpaired, state(paired = null).connection)
    }

    @Test
    fun a_tls_endpoint_is_paired_and_shows_the_grouped_pin() {
        val connection = state(
            paired = SyncSettings("https://10.0.2.2:8765", "tok", realPin),
        ).connection
        assertTrue(connection is ConnectionView.Paired)
        connection as ConnectionView.Paired
        assertEquals("https://10.0.2.2:8765", connection.endpoint)
        assertEquals(
            PinView.Pinned("94e3 2367 cdca 1736 50e3 dbd7 3f4a 2dc6 57e0 951c 9cc0 9745 56a6 909f e521 6c3c"),
            connection.pin,
        )
    }

    @Test
    fun a_cleartext_endpoint_is_paired_but_unpinned() {
        // The --no-tls fallback: paired, but there is no cert to pin, and the card must say so
        // rather than implying a verified pin.
        val connection = state(
            paired = SyncSettings("http://10.0.2.2:8765", "tok", pin = null),
        ).connection
        assertEquals(ConnectionView.Paired("http://10.0.2.2:8765", PinView.Cleartext), connection)
    }

    @Test
    fun the_active_model_is_labelled_on_device() {
        assertEquals(
            "base.en · on-device",
            state(model = InstalledModel("base.en", "sha", "/path/base.en.bin")).modelLabel,
        )
    }

    @Test
    fun a_known_catalog_model_shows_a_friendly_name_and_size() {
        // A model from the public catalog reads as its picker label + size, not the raw ggml id.
        assertEquals(
            "Tiny (English) · on-device · 31 MB",
            state(model = InstalledModel("ggml-tiny.en-q5_1", "sha", "/p/ggml-tiny.en-q5_1.bin")).modelLabel,
        )
    }

    @Test
    fun no_model_is_stated_plainly() {
        assertEquals("No model yet", state(model = null).modelLabel)
    }

    @Test
    fun the_toggles_reflect_the_persisted_prefs() {
        val view = state(
            prefs = PrefsSnapshot(
                reviewByDefault = true,
                continuousOnDoubleTap = false,
                shipCorrections = false,
                quickLaunchMic = false,
            ),
        )
        assertTrue(view.reviewOn)
        assertEquals(false, view.continuousOn)
        assertEquals(false, view.shipOn)
        assertEquals(false, view.quickLaunchOn)
    }

    @Test
    fun quick_launch_pref_drives_the_view() {
        assertTrue(
            state(
                prefs = PrefsSnapshot(
                    reviewByDefault = false,
                    continuousOnDoubleTap = true,
                    shipCorrections = true,
                    quickLaunchMic = true,
                ),
            ).quickLaunchOn,
        )
    }

    @Test
    fun the_audio_row_formats_usage_against_the_cap() {
        assertEquals(
            "214 MB of 1.0 GB",
            state(audioUsedBytes = 214L * 1024 * 1024, audioCapBytes = 1024L * 1024 * 1024).audioLabel,
        )
    }

    @Test
    fun the_system_status_passes_through_for_the_status_rows() {
        val system = SystemStatus(keyboardEnabled = true, keyboardSelected = false, micGranted = false)
        assertEquals(system, state(system = system).system)
    }
}
