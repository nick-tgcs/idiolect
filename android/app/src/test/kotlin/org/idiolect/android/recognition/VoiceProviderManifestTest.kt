package org.idiolect.android.recognition

import android.content.Context
import android.content.Intent
import android.speech.RecognitionService
import android.speech.RecognizerIntent
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * The user's headline bug: in an app's built-in voice/mic picker (a browser search box, etc.)
 * idiolect was **not** an option — only Google and a sibling app showed up. To appear, idiolect
 * must register the two standard speech surfaces, so this is a pure on-manifest guard (mirrors
 * [org.idiolect.android.setup.PairingDeepLinkManifestTest]) that a future edit can't silently
 * drop the registration; the recognition the components then perform is covered by the session
 * unit tests and the connected e2e.
 *
 *  1. An Activity for `ACTION_RECOGNIZE_SPEECH` — what an in-app mic button fires
 *     (`startActivityForResult`); this is the picker the user saw.
 *  2. A [RecognitionService] — so idiolect is selectable as the *system* speech engine and
 *     usable by the `SpeechRecognizer` API.
 */
@RunWith(RobolectricTestRunner::class)
class VoiceProviderManifestTest {
    private val pkg = "org.idiolect.android"

    @Test
    fun idiolect_handles_recognize_speech_so_it_appears_in_an_app_mic_picker() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val resolved = context.packageManager
            .queryIntentActivities(Intent(RecognizerIntent.ACTION_RECOGNIZE_SPEECH), 0)
        assertTrue(
            "no idiolect activity handles ACTION_RECOGNIZE_SPEECH — it can't appear as a mic option",
            resolved.any { it.activityInfo.packageName == pkg },
        )
    }

    @Test
    fun idiolect_registers_a_system_recognition_service() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val resolved = context.packageManager
            .queryIntentServices(Intent(RecognitionService.SERVICE_INTERFACE), 0)
        assertTrue(
            "no idiolect RecognitionService — idiolect can't be the system speech engine",
            resolved.any { it.serviceInfo.packageName == pkg },
        )
    }
}
