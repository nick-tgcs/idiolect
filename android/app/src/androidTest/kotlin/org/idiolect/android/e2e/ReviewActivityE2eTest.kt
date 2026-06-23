package org.idiolect.android.e2e

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
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * End-to-end for the centred review surface (the 👁 flow): the [ReviewActivity] floats over
 * the screen with the transcript in an **editable** field, and **Insert** stashes the
 * (possibly edited) text for the IME to type on return ([PendingInsert]) — while **Cancel**
 * stashes nothing. Capturing the training pair is covered at the FFI/integration level (it
 * needs a real persisted take id); here we drive the real Activity + window for the UI +
 * deferred-insert plumbing.
 */
@RunWith(AndroidJUnit4::class)
class ReviewActivityE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)

    @Before
    fun clearPending() {
        // This class exercises the deferred-insert fallback (instant insert OFF), so Insert
        // stashes into PendingInsert. Force the accessibility service off so a prior test that
        // enabled it can't flip this into the injection path (which stashes nothing).
        device.executeShellCommand("settings delete secure enabled_accessibility_services")
        device.executeShellCommand("settings put secure accessibility_enabled 0")
        PendingInsert.take()
    }

    @After
    fun dismiss() {
        PendingInsert.take()
        device.pressHome()
    }

    @Test
    fun the_review_card_shows_the_transcript_editably() {
        launchReview("restart the traffic service")
        assertNotNull("the transcript field is missing", device.findObject(By.text("restart the traffic service")))
        assertNotNull("Insert is missing", device.findObject(By.text("Insert")))
        assertNotNull("Cancel is missing", device.findObject(By.text("Cancel")))
    }

    @Test
    fun insert_stashes_the_edited_text_for_the_ime() {
        launchReview("hello there")
        // Find the editable field by class (its text changes as we edit, so By.text is flaky).
        val field = device.wait(Until.findObject(By.clazz("android.widget.EditText")), TIMEOUT)
        assertNotNull("review edit field missing", field)
        field.text = "hello world" // edit it (as the user would, with their own keyboard)
        device.findObject(By.text("Insert")).click()
        device.waitForIdle()
        assertEquals("Insert must stash the edited text", "hello world", PendingInsert.take())
    }

    @Test
    fun cancel_stashes_nothing() {
        launchReview("discard me")
        device.findObject(By.text("Cancel")).click()
        device.waitForIdle()
        assertNull("Cancel must not stash any text", PendingInsert.take())
    }

    private fun launchReview(text: String) {
        // -1 id: no persisted take to amend (capture is a no-op here); the UI + plumbing is
        // what we exercise. The real raw→corrected capture is integration-tested.
        ReviewActivity.launch(instrumentation.targetContext, -1L, text)
        assertTrue(
            "the review card never appeared",
            device.wait(Until.hasObject(By.textContains("Review dictation")), TIMEOUT),
        )
        // The field auto-focuses, so a keyboard pops up over the card's buttons. Dismiss it
        // (one Back hides the keyboard, not the Activity) so Insert/Cancel are reachable.
        device.waitForIdle()
        device.pressBack()
        device.wait(Until.hasObject(By.text("Insert")), TIMEOUT)
    }

    companion object {
        private const val TIMEOUT = 8_000L
    }
}
