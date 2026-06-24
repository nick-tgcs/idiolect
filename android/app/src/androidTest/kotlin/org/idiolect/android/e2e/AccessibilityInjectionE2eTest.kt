package org.idiolect.android.e2e

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.idiolect.android.accessibility.IdiolectAccessibilityService
import org.idiolect.android.accessibility.InjectQueue
import org.idiolect.android.ime.PendingInsert
import org.idiolect.android.ime.ReviewActivity
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.File

/**
 * End-to-end for the **instant-insert hand-off** (the 👁 flow the user asked for): when the
 * accessibility service is enabled, the review dialog's Insert writes the corrected text into
 * the cross-process [InjectQueue] the service drains — rather than deferring to the IME — so a
 * fix made with the user's own keyboard lands in the app with no keyboard switch.
 *
 * What this asserts is the dialog's routing on a real device + window: enabled → queue (here),
 * disabled → deferred ([ReviewActivityE2eTest]). The **other half — the bound service draining
 * the queue into another app's field — can't run under instrumentation**: Android won't bind an
 * accessibility service for a package that's currently under `am instrument` (the bind is
 * simply never attempted). That half is verified on-device (a reviewed take injected "trafic"
 * straight into a second app's field) and unit-tested where it has a seam: the splice
 * ([TextInjectionTest]), the target rule ([InjectionTargetingTest]), the queue
 * ([InjectQueueTest]), and the enabled-parse ([AccessibilityServicesTest]).
 */
@RunWith(AndroidJUnit4::class)
class AccessibilityInjectionE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)
    private val targetContext get() = instrumentation.targetContext
    private val queue get() = InjectQueue(File(targetContext.filesDir, IdiolectAccessibilityService.PENDING_FILE))

    @Before
    fun enableInstantInsert() {
        PendingInsert.take()
        queue.take() // drain any stale value
        // The dialog reads this setting to decide queue-vs-defer. We only need it *listed*
        // (the service binding itself can't happen under instrumentation — see the class doc).
        shell("settings put secure enabled_accessibility_services $SERVICE_ID")
    }

    @After
    fun disableInstantInsert() {
        shell("settings delete secure enabled_accessibility_services")
        shell("settings put secure accessibility_enabled 0")
        PendingInsert.take()
        queue.take()
        device.pressHome()
    }

    @Test
    fun insert_queues_the_reviewed_text_for_the_service() {
        ReviewActivity.launch(targetContext, -1L, "restart the trafic service")
        assertNotNull(
            "the review card never appeared",
            device.wait(Until.findObject(By.textContains("Review dictation")), TIMEOUT),
        )

        // Fix it as the user would, with their keyboard (UI Automator sets the field text).
        val field = device.wait(Until.findObject(By.clazz("android.widget.EditText").pkg(APP_PKG)), TIMEOUT)
        assertNotNull("review edit field missing", field)
        field.text = "restart the traffic service"
        device.findObject(By.text("Insert")).click()
        device.waitForIdle()

        // With instant insert on, the corrected text is handed to the service via the queue —
        // NOT deferred to the IME. The service (verified on-device) drains it into the field.
        assertEquals(
            "Insert must queue the corrected text for the accessibility service",
            "restart the traffic service",
            queue.take(),
        )
        assertNull("it must not also defer to the IME", PendingInsert.take())
    }

    private fun shell(command: String) {
        device.executeShellCommand(command)
    }

    companion object {
        private const val APP_PKG = "org.idiolect.android"
        private const val SERVICE_ID =
            "$APP_PKG/$APP_PKG.accessibility.IdiolectAccessibilityService"
        private const val TIMEOUT = 8_000L
    }
}
