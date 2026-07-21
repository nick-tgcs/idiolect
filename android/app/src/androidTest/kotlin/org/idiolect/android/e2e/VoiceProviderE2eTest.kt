package org.idiolect.android.e2e

import android.content.Intent
import android.speech.RecognitionService
import android.speech.RecognizerIntent
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import androidx.test.uiautomator.By
import androidx.test.uiautomator.UiDevice
import androidx.test.uiautomator.Until
import org.idiolect.android.recognition.RecognizeSpeechActivity
import org.junit.After
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * End-to-end on the emulator that idiolect is a real voice / mic provider on the device — the
 * user's headline bug (idiolect was missing from an app's mic picker, only Google + a sibling
 * app showed). Unlike the Robolectric [org.idiolect.android.recognition.VoiceProviderManifestTest]
 * this checks the **installed, merged** manifest, then launches the RECOGNIZE_SPEECH activity and
 * asserts idiolect's own listening surface comes up (so the registration isn't just declared but
 * actually handles the intent).
 */
@RunWith(AndroidJUnit4::class)
class VoiceProviderE2eTest {
    private val instrumentation get() = InstrumentationRegistry.getInstrumentation()
    private val device: UiDevice get() = UiDevice.getInstance(instrumentation)
    private val ctx get() = instrumentation.targetContext

    @Before
    fun grantMic() {
        device.executeShellCommand("pm grant $PKG android.permission.RECORD_AUDIO")
    }

    @After
    fun close() {
        device.pressBack()
        device.pressHome()
    }

    @Test
    fun idiolect_is_a_registered_voice_input_option_on_the_device() {
        val activities = ctx.packageManager
            .queryIntentActivities(Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH), 0)
        assertTrue(
            "idiolect's RECOGNIZE_SPEECH activity must be installed so apps list it as a mic option",
            activities.any { it.activityInfo.packageName == PKG },
        )
        val services = ctx.packageManager
            .queryIntentServices(Intent(RecognitionService.SERVICE_INTERFACE), 0)
        assertTrue(
            "idiolect's RecognitionService must be installed so it's selectable as the speech engine",
            services.any { it.serviceInfo.packageName == PKG },
        )
    }

    @Test
    fun launching_recognize_speech_brings_up_idiolects_listening_surface() {
        // Explicit component so the system picker (Google also handles the action) is skipped and
        // the assertion is deterministic; the surface renders whether or not a model is installed.
        ctx.startActivity(
            Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
                .setClassName(PKG, "$PKG.recognition.RecognizeSpeechActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        assertTrue(
            "idiolect's voice surface (the mic) never appeared",
            device.wait(Until.hasObject(By.desc(RecognizeSpeechActivity.MIC_DESC)), TIMEOUT),
        )
    }

    @Test
    fun a_silent_take_finalizes_and_dismisses_instead_of_hanging() {
        // The core sends no commitText/dictationError for a silent take — only recordingStatus(false).
        // With the emulator mic silent, tapping "done" must still finalize (=> no-speech) and close,
        // not leave the surface stuck on "Transcribing…". Needs the installed model to start a take.
        ctx.startActivity(
            Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH)
                .setClassName(PKG, "$PKG.recognition.RecognizeSpeechActivity")
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
        assertTrue(
            "the take never reached Listening (no model installed?)",
            device.wait(Until.hasObject(By.textContains("Listening")), MODEL_TIMEOUT),
        )
        // Say nothing, tap the surface to finish.
        device.click(device.displayWidth / 2, device.displayHeight / 2)
        assertTrue(
            "a silent recognize take must finalize and dismiss, not hang on Transcribing…",
            device.wait(Until.gone(By.desc(RecognizeSpeechActivity.MIC_DESC)), TIMEOUT),
        )
    }

    companion object {
        private const val PKG = "org.idiolect.android"
        private const val TIMEOUT = 10_000L

        /** Generous wait covering the on-device model load before the take reaches "Listening". */
        private const val MODEL_TIMEOUT = 25_000L
    }
}
