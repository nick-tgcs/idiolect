package org.idiolect.android.e2e

import android.content.Intent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * End-to-end on the emulator for the flanking edit keys (Option A): the ⌫ delete and ⏎ enter
 * keys that sit either side of the circular mic. Drives the real idiolect IME over a real
 * multiline [EditorHarnessActivity] field via UI Automator, asserting the observable
 * behaviour through the live `InputConnection`:
 *   - the two keys render *alongside* the mic + control strip (new keys, old surface intact),
 *   - ⌫ deletes the character before the cursor,
 *   - ⏎ inserts a newline in a field with no declared IME action (the common dictation case).
 *
 * Out of scope here (covered deterministically by JVM tests, since observing an editor action
 * headless would need a listener wired into the harness): the ⏎ *action* branch — a search/
 * send/done field performing its IME action instead of a newline — is unit-tested in
 * [org.idiolect.android.ime.EnterActionTest] and wired through in
 * [org.idiolect.android.ime.EditKeysTest].
 */
@RunWith(AndroidJUnit4::class)
class EditKeysE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)

    @Before
    fun selectIdiolectAndFocusAField() {
        shell("pm grant $PKG android.permission.RECORD_AUDIO")
        shell("ime enable $IME_ID")
        shell("ime set $IME_ID")

        val ctx = instrumentation.context
        ctx.startActivity(
            Intent(ctx, EditorHarnessActivity::class.java)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK),
        )
        assertTrue(
            "harness field never appeared",
            device.wait(Until.hasObject(By.desc(EditorHarnessActivity.FIELD_DESC)), TIMEOUT),
        )
        field().click()
        ensureVoiceMode()
    }

    @After
    fun dismiss() {
        device.pressBack()
        device.pressHome()
    }

    private fun ensureVoiceMode() {
        repeat(4) {
            if (device.wait(Until.hasObject(By.descContains(MIC_DESC)), 2_500)) return
            shell("ime set $IME_ID")
            field().click()
        }
        assertTrue(
            "the idiolect voice keyboard never appeared",
            device.hasObject(By.descContains(MIC_DESC)),
        )
    }

    @Test
    fun the_mic_surface_shows_the_delete_and_enter_keys_alongside_the_mic() {
        // New keys and the old surface coexist: the mic is still central, flanked by ⌫ and ⏎.
        assertNotNull("mic missing", device.findObject(By.descContains(MIC_DESC)))
        assertNotNull("delete key missing", device.findObject(By.desc(DELETE_DESC)))
        assertNotNull("enter key missing", device.findObject(By.desc(ENTER_DESC)))
    }

    @Test
    fun the_delete_key_removes_the_character_before_the_cursor() {
        setField("abc")
        device.findObject(By.desc(DELETE_DESC)).click()
        waitForFieldText("ab")
    }

    @Test
    fun the_enter_key_inserts_a_newline_in_a_field_with_no_ime_action() {
        setField("ab")
        device.findObject(By.desc(ENTER_DESC)).click()
        waitForFieldText("ab\n")
    }

    private fun field() = device.findObject(By.desc(EditorHarnessActivity.FIELD_DESC))

    private fun setField(text: String) {
        field().text = text
        waitForFieldText(text)
    }

    /** The keys post their edit to the main thread, so poll the field until it settles. */
    private fun waitForFieldText(expected: String) {
        repeat(40) {
            if ((field()?.text ?: "") == expected) return
            Thread.sleep(100)
        }
        assertEquals(expected, field()?.text ?: "")
    }

    private fun shell(command: String) {
        device.executeShellCommand(command)
    }

    companion object {
        private const val PKG = "org.idiolect.android"
        private const val IME_ID = "$PKG/.ime.IdiolectImeService"
        private const val MIC_DESC = "Dictation microphone"
        private const val DELETE_DESC = "Delete"
        private const val ENTER_DESC = "Enter"
        private const val TIMEOUT = 12_000L
    }
}
