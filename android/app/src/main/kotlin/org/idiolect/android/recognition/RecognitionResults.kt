package org.idiolect.android.recognition

import android.os.Bundle
import android.speech.SpeechRecognizer

/**
 * The result envelope handed back by both voice surfaces. An `ACTION_RECOGNIZE_SPEECH` caller
 * reads [SpeechRecognizer.RESULTS_RECOGNITION] (an ordered list of hypotheses, best first); a
 * [android.speech.RecognitionService] `Callback.results(...)` carries the same list plus a
 * parallel confidence array. idiolect emits a single hypothesis from the on-device decode.
 */
object RecognitionResults {
    /** The hypothesis list an `EXTRA_RESULTS` / `RESULTS_RECOGNITION` consumer expects. */
    fun list(text: String): ArrayList<String> = arrayListOf(text)

    /** The Bundle a `RecognitionService.Callback.results(...)` delivers for [text]. */
    fun bundle(text: String): Bundle = Bundle().apply {
        putStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION, list(text))
        putFloatArray(SpeechRecognizer.CONFIDENCE_SCORES, floatArrayOf(1f))
    }
}
