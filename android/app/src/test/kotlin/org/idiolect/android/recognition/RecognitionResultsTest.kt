package org.idiolect.android.recognition

import android.speech.SpeechRecognizer
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * The result envelope both voice surfaces hand back: an `ACTION_RECOGNIZE_SPEECH` caller reads
 * the [SpeechRecognizer.RESULTS_RECOGNITION] string list, and a [android.speech.RecognitionService]
 * `Callback.results(...)` carries the same list (plus a confidence) in a Bundle. Pinned here so
 * the exact keys an app expects can't drift.
 */
@RunWith(RobolectricTestRunner::class)
class RecognitionResultsTest {
    @Test
    fun the_results_bundle_carries_the_transcript_under_the_standard_key() {
        val bundle = RecognitionResults.bundle("hello world")
        assertEquals(
            arrayListOf("hello world"),
            bundle.getStringArrayList(SpeechRecognizer.RESULTS_RECOGNITION),
        )
    }

    @Test
    fun the_results_bundle_carries_a_confidence_score_per_hypothesis() {
        val bundle = RecognitionResults.bundle("hello world")
        val scores = bundle.getFloatArray(SpeechRecognizer.CONFIDENCE_SCORES)
        assertEquals(1, scores?.size)
    }
}
