package org.idiolect.android.e2e

import android.provider.Settings
import android.view.inputmethod.InputMethodManager
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.idiolect.android.ime.PendingInsert
import org.idiolect.android.ime.ReviewActivity
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeNotNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * End-to-end for the **auto-return** the user asked for: after Insert, idiolect makes itself the
 * active IME again (writing `Settings.Secure.DEFAULT_INPUT_METHOD`) so its mic comes back with no
 * manual keyboard switch. Android forbids a plain app from selecting an IME *without*
 * `WRITE_SECURE_SETTINGS`, so the production app needs a one-time `adb pm grant`; this test grants
 * it the same way and observes the setting flip to idiolect's id.
 *
 * **Good-citizen cleanup matters here.** Switching/enabling IMEs and the `WRITE_SECURE_SETTINGS`
 * grant are *global, process-persistent* state shared with the sibling e2e classes on the one
 * emulator. So this class restores the active IME (waiting until it actually settles — idiolect's
 * mic has no real keyboard, and leaving it active breaks a field that needs to type), revokes the
 * grant (else a later [ReviewActivityE2eTest] Insert would also fire the auto-switch and thrash
 * the IME mid-suite), and leaves idiolect's enabled-state as it found it.
 *
 * Three-level note: the FFI/integration tier doesn't apply — this path never touches the Rust
 * core, only Android settings — so it's covered at the unit
 * ([org.idiolect.android.ime.ImeSelectionTest], the id format) and e2e (here, the real Activity
 * writing the real setting) levels.
 */
@RunWith(AndroidJUnit4::class)
class ImeReturnE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)
    private val targetContext get() = instrumentation.targetContext

    /**
     * idiolect's IME id in the framework's own short form (`pkg/.ime.Class`) — the form
     * `ime enable/set` and `DEFAULT_INPUT_METHOD` recognise (the long form is rejected as
     * "Unknown id"). Used as a literal here, exactly as the sibling [VoiceKeyboardE2eTest]
     * does, because the short id is deterministic for this package and is needed *before*
     * idiolect is enabled (so it can't be read back from the enabled-IME list yet).
     */
    private val idiolectIme: String
        get() = "${targetContext.packageName}/.ime.IdiolectImeService"

    private var originalDefaultIme: String? = null

    @Before
    fun grantAndForceAnotherKeyboard() {
        // The whole point of this feature: idiolect can only auto-select itself with this grant.
        shell("pm grant ${targetContext.packageName} android.permission.WRITE_SECURE_SETTINGS")
        originalDefaultIme = currentDefaultIme()
        // idiolect's IME must be enabled for the system to honour it as the default (otherwise the
        // IMMS reverts an invalid default asynchronously and the assertion races). We deliberately
        // do NOT disable it again in cleanup: an enable→disable→re-enable cycle leaves the IMM's
        // switch state confused enough to break the sibling [VoiceKeyboardE2eTest] handoff, and a
        // left-enabled IME is benign (that class enables it anyway).
        shell("ime enable $idiolectIme")
        PendingInsert.take()

        // Switch the active IME to some *other* enabled keyboard, so "it became idiolect after
        // Insert" is a real transition, not a no-op. Skip the test if idiolect is the only IME.
        val other = otherEnabledIme()
        assumeNotNull("no non-idiolect IME enabled to switch away from", other)
        shell("ime set $other")
        assertTrue("precondition: a non-idiolect IME is active", waitForActiveIme(other!!))
    }

    @After
    fun restore() {
        PendingInsert.take()
        // Revoke first so the grant can't leak into a sibling test's Insert.
        shell("pm revoke ${targetContext.packageName} android.permission.WRITE_SECURE_SETTINGS")
        // Switch away from idiolect to a real keyboard and WAIT until it has settled, so the next
        // class doesn't start mid-transition (idiolect may be the active IME now — Insert set it).
        originalDefaultIme?.let { ime ->
            shell("ime set $ime")
            waitForActiveIme(ime)
        }
        device.waitForIdle()
        device.pressHome()
    }

    @Test
    fun insert_makes_idiolect_the_active_ime_again() {
        ReviewActivity.launch(targetContext, -1L, "back to the mic")
        assertNotNull(
            "the review card never appeared",
            device.wait(Until.findObject(By.textContains("Review dictation")), TIMEOUT),
        )

        device.findObject(By.text("Insert")).click()

        assertTrue(
            "Insert must pull the active IME back to idiolect (auto-return)",
            waitForActiveIme(idiolectIme),
        )
    }

    /** Poll until the framework reports [expected] as the active IME (the switch is async). */
    private fun waitForActiveIme(expected: String): Boolean {
        val deadline = System.nanoTime() + TIMEOUT * 1_000_000
        while (currentDefaultIme() != expected && System.nanoTime() < deadline) {
            device.waitForIdle()
        }
        return currentDefaultIme() == expected
    }

    /** The ids of every currently enabled IME. */
    private fun enabledImeIds(): List<String> =
        targetContext.getSystemService(InputMethodManager::class.java).enabledInputMethodList.map { it.id }

    /** An enabled IME that isn't idiolect, or null if idiolect is the only enabled keyboard. */
    private fun otherEnabledIme(): String? = enabledImeIds().firstOrNull { it != idiolectIme }

    private fun currentDefaultIme(): String? =
        Settings.Secure.getString(targetContext.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)

    private fun shell(command: String) {
        device.executeShellCommand(command)
    }

    companion object {
        private const val TIMEOUT = 8_000L
    }
}
