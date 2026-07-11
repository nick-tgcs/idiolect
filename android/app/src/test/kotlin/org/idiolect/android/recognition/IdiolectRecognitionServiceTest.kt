package org.idiolect.android.recognition

import android.Manifest
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * The system [android.speech.RecognitionService] path is headless — it can't pop a permission
 * prompt — so a take must be refused up front when the mic isn't granted (the caller then gets
 * `ERROR_INSUFFICIENT_PERMISSIONS`, mapped in [RecognitionErrorCodesTest]) rather than failing
 * later as a generic recognition error. Mic is checked ahead of the model. Pinned to SDK 33 to
 * match the other service tests. The `Callback` wiring itself is covered by the connected e2e.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class IdiolectRecognitionServiceTest {
    private fun service(): IdiolectRecognitionService =
        Robolectric.buildService(IdiolectRecognitionService::class.java).create().get()

    @Test
    fun a_take_without_mic_permission_is_blocked_as_a_permission_error() {
        shadowOf(RuntimeEnvironment.getApplication())
            .denyPermissions(Manifest.permission.RECORD_AUDIO)

        assertEquals(RecognitionError.MIC_PERMISSION, service().startBlocker())
    }

    @Test
    fun with_the_mic_granted_a_missing_model_is_what_blocks() {
        shadowOf(RuntimeEnvironment.getApplication())
            .grantPermissions(Manifest.permission.RECORD_AUDIO)

        // No model installed in the test's fresh filesDir, so the mic clears and the model blocks.
        assertEquals(RecognitionError.MODEL_MISSING, service().startBlocker())
    }
}
