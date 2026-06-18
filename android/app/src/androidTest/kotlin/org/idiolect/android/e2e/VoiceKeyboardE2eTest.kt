package org.idiolect.android.e2e

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.junit.After
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * End-to-end on the emulator: drives the real idiolect IME over a real text field via UI
 * Automator, asserting the redesigned voice keyboard's observable behaviour — the circular
 * mic + control strip render, a tap starts a live take ("Listening…"), and the ⌨ button
 * hands the field to the user's own keyboard (idiolect has no keyboard of its own). The
 * core's recording edge fires before audio capture, so these hold without a speech model.
 *
 * Out of scope here (covered deterministically by JVM unit tests, and not reachable
 * headless without a model + audio injection): the hold-vs-double-tap *timing*
 * discrimination ([org.idiolect.android.ime.MicGestureRecognizerTest]), the switch fallback
 * order ([org.idiolect.android.ime.KeyboardHandoffTest]), and transcription from a real take
 * (needs a model + spoken audio the emulator can't inject).
 */
@RunWith(AndroidJUnit4::class)
class VoiceKeyboardE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)

    @Before
    fun selectIdiolectAndFocusAField() {
        shell("pm grant $PKG android.permission.RECORD_AUDIO")
        shell("ime enable $IME_ID")
        shell("ime set $IME_ID")

        // The harness activity is declared in the androidTest APK, so launch it from the
        // instrumentation (test) context — not the app-under-test's targetContext.
        val ctx = instrumentation.context
        ctx.startActivity(
            Intent(ctx, EditorHarnessActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
        )
        // Tap the field to focus it and bring idiolect up.
        assertTrue(
            "harness field never appeared",
            device.wait(Until.hasObject(By.desc(EditorHarnessActivity.FIELD_DESC)), TIMEOUT),
        )
        device.findObject(By.desc(EditorHarnessActivity.FIELD_DESC)).click()
        ensureVoiceMode()
    }

    @After
    fun dismiss() {
        device.pressBack()
        device.pressHome()
    }

    /**
     * A prior test may have handed off to another keyboard — re-select idiolect (the @Before
     * `ime set` already did, but re-tapping the field guarantees its view is up) so every
     * test starts from the mic.
     */
    private fun ensureVoiceMode() {
        repeat(4) {
            if (device.wait(Until.hasObject(By.descContains(MIC_DESC)), 2_500)) return
            shell("ime set $IME_ID")
            device.findObject(By.desc(EditorHarnessActivity.FIELD_DESC))?.click()
        }
        assertTrue(
            "the idiolect voice keyboard never appeared",
            device.hasObject(By.descContains(MIC_DESC)),
        )
    }

    @Test
    fun the_voice_keyboard_renders_the_mic_and_control_strip() {
        assertNotNull("mic missing", device.findObject(By.descContains(MIC_DESC)))
        assertNotNull("switch-to-keyboard button missing", device.findObject(By.desc(SWITCH_DESC)))
        assertNotNull("review button missing", device.findObject(By.desc("Review before insert")))
        assertNotNull("idle hint missing", device.findObject(By.textContains("Hold to talk")))
    }

    @Test
    fun a_tap_on_the_mic_starts_a_live_take() {
        device.findObject(By.descContains(MIC_DESC)).click()
        // The lone tap is confirmed after the double-tap window, then the take starts and the
        // core pushes recording → the status line shows "Listening…".
        assertTrue(
            "tapping the mic did not start listening",
            device.wait(Until.hasObject(By.textContains("Listening")), TIMEOUT),
        )
        // Stop the take so it doesn't bleed into the next test (tap again = stop).
        device.findObject(By.descContains(MIC_DESC)).click()
        device.wait(Until.gone(By.textContains("Listening")), TIMEOUT)
    }

    @Test
    fun the_keyboard_button_hands_off_to_the_users_own_keyboard() {
        // idiolect has no keyboard of its own — ⌨ switches to the user's keyboard via the
        // system IME switch, so the idiolect mic disappears (another IME takes over).
        device.findObject(By.desc(SWITCH_DESC)).click()
        assertTrue(
            "tapping ⌨ did not hand off to another keyboard (mic still showing)",
            device.wait(Until.gone(By.descContains(MIC_DESC)), TIMEOUT),
        )
        // And the system's active IME really changed away from idiolect — not just the view
        // hiding. (Regression guard: `switchToNextInputMethod` returns false with no switch
        // history, so this only holds because we switch to a specific IME by id.)
        val active = shellOut("settings get secure default_input_method")
        assertFalse(
            "the active IME is still idiolect after ⌨ ($active)",
            active.contains(PKG),
        )
        // Restore idiolect so the next test starts from the mic.
        shell("ime set $IME_ID")
    }

    private fun shell(command: String) {
        device.executeShellCommand(command)
    }

    private fun shellOut(command: String): String = device.executeShellCommand(command)

    companion object {
        private const val PKG = "org.idiolect.android"
        private const val IME_ID = "$PKG/.ime.IdiolectImeService"
        private const val MIC_DESC = "Dictation microphone"
        private const val SWITCH_DESC = "Switch to your keyboard"
        private const val TIMEOUT = 12_000L
    }
}
