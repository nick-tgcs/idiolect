package org.idiolect.android.e2e

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.idiolect.android.settings.SettingsActivity
import org.idiolect.android.settings.SettingsStore
import org.idiolect.android.sync.PairingTokenStore
import org.idiolect.android.sync.SecureSyncConfig
import org.idiolect.android.sync.SyncSettings
import org.junit.After
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * End-to-end on the emulator for the settings screen reached via the ⚙ on the mic strip. It
 * launches the **real** [SettingsActivity] through the same `launch()` entry the IME service
 * uses and drives it with UI Automator, asserting the device-only behaviours that have no
 * headless seam:
 *
 *  - an unpaired device shows the "Scan QR to pair" call to action;
 *  - a paired endpoint shows its URL and its pin as "verified by QR";
 *  - tapping **Unpair** wipes the endpoint through the *real* AndroidKeyStore-backed
 *    [SecureSyncConfig] (the security-critical new path: no stale token/pin survives);
 *  - toggling "Review before insert" persists to the real [SettingsStore] file.
 *
 * The camera QR capture itself has no headless seam (as in `SetupActivity`), so the actual
 * scan stays covered by `PairingDeepLinkE2eTest`; here the scan button only needs to be present.
 * Needs no microphone (no dictation), so it runs on a `-no-audio` AVD, sidestepping the mic hazard.
 */
@RunWith(AndroidJUnit4::class)
class SettingsScreenE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)
    private val ctx get() = instrumentation.targetContext

    @Before
    fun clearPersistedState() {
        // Start every case from a clean, unpaired, default-toggles device.
        File(ctx.filesDir, SecureSyncConfig.URL_FILE_NAME).delete()
        File(ctx.filesDir, SecureSyncConfig.PIN_FILE_NAME).delete()
        File(ctx.filesDir, PairingTokenStore.FILE_NAME).delete()
        File(ctx.filesDir, SettingsStore.FILE_NAME).delete()
    }

    @After
    fun close() {
        device.pressBack()
        device.pressHome()
    }

    @Test
    fun an_unpaired_device_shows_the_scan_call_to_action() {
        SettingsActivity.launch(ctx)
        assertTrue(
            "the unpaired connection card must offer scan-to-pair",
            device.wait(Until.hasObject(By.text("Scan QR to pair")), TIMEOUT),
        )
    }

    @Test
    fun a_paired_endpoint_shows_its_pin_and_unpair_wipes_it() {
        val config = SecureSyncConfig.keystoreBacked(ctx.filesDir)
        config.save(SyncSettings("https://10.0.2.2:8765", "tok-e2e", PIN))

        SettingsActivity.launch(ctx)

        assertTrue(
            "the paired card shows the endpoint",
            device.wait(Until.hasObject(By.text("https://10.0.2.2:8765")), TIMEOUT),
        )
        assertNotNull(
            "the pin is presented as verified by QR",
            device.findObject(By.textStartsWith("Pinned certificate")),
        )
        assertNotNull(
            "the grouped fingerprint is shown for human verification",
            device.findObject(By.textContains("dead beef")),
        )

        val unpair = device.wait(Until.findObject(By.text("Unpair")), TIMEOUT)
        assertNotNull("the paired card offers Unpair", unpair)
        unpair.click()

        // Unpair runs off the UI thread; poll the real keystore-backed config until it clears.
        var waited = 0L
        while (config.load() != null && waited < TIMEOUT) {
            Thread.sleep(POLL_MS)
            waited += POLL_MS
        }
        assertNull("Unpair must wipe the endpoint, token, and pin together", config.load())
    }

    @Test
    fun toggling_review_before_insert_persists_to_the_store() {
        val store = SettingsStore.under(ctx.filesDir)
        assertTrue("precondition: review defaults off", !store.reviewByDefault())

        SettingsActivity.launch(ctx)
        // The first Switch top-to-bottom is "Review before insert" (Dictation is the first card
        // with a toggle); flip it and assert the change reached the persisted store.
        assertTrue(
            "the dictation toggles are shown",
            device.wait(Until.hasObject(By.clazz("android.widget.Switch")), TIMEOUT),
        )
        device.findObjects(By.clazz("android.widget.Switch")).first().click()

        var waited = 0L
        while (!store.reviewByDefault() && waited < TIMEOUT) {
            Thread.sleep(POLL_MS)
            waited += POLL_MS
        }
        assertTrue("flipping the review switch persists the default", store.reviewByDefault())
    }

    companion object {
        private const val TIMEOUT = 10_000L
        private const val POLL_MS = 200L

        /** A 64-hex-char fingerprint; its grouped form contains "dead beef" quads. */
        private val PIN = "deadbeef".repeat(8)
    }
}
