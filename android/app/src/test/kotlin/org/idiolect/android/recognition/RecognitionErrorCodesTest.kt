package org.idiolect.android.recognition

import android.speech.SpeechRecognizer
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The bridge from idiolect's [RecognitionError] to the `SpeechRecognizer.ERROR_*` code a
 * [android.speech.RecognitionService] caller switches on. Pinned so a caller gets a sensible,
 * stable code (e.g. silence ⇒ "no match", not a generic client error) and every error stays
 * distinguishable.
 */
class RecognitionErrorCodesTest {
    @Test
    fun silence_maps_to_no_match() {
        assertEquals(
            SpeechRecognizer.ERROR_NO_MATCH,
            RecognitionErrorCodes.speechRecognizer(RecognitionError.NO_SPEECH),
        )
    }

    @Test
    fun a_missing_mic_permission_maps_to_insufficient_permissions() {
        assertEquals(
            SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS,
            RecognitionErrorCodes.speechRecognizer(RecognitionError.MIC_PERMISSION),
        )
    }

    @Test
    fun every_error_has_a_distinct_code() {
        val codes = RecognitionError.entries.map { RecognitionErrorCodes.speechRecognizer(it) }
        assertEquals("each RecognitionError must map to a unique code", codes.size, codes.toSet().size)
    }
}
