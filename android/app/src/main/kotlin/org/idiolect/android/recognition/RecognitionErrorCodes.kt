package org.idiolect.android.recognition

import android.speech.SpeechRecognizer

/**
 * Maps idiolect's [RecognitionError] to the `SpeechRecognizer.ERROR_*` code a
 * [android.speech.RecognitionService] `Callback.error(...)` reports. Kept pure (a `when` over the
 * enum) so the choice of code per failure is unit-pinned rather than scattered in the service.
 */
object RecognitionErrorCodes {
    fun speechRecognizer(error: RecognitionError): Int = when (error) {
        RecognitionError.MIC_PERMISSION -> SpeechRecognizer.ERROR_INSUFFICIENT_PERMISSIONS
        RecognitionError.MODEL_MISSING -> SpeechRecognizer.ERROR_SERVER
        RecognitionError.NO_SPEECH -> SpeechRecognizer.ERROR_NO_MATCH
        RecognitionError.FAILED -> SpeechRecognizer.ERROR_CLIENT
    }
}
