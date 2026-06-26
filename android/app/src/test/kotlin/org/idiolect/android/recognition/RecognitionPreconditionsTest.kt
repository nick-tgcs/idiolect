package org.idiolect.android.recognition

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * What stops a recognition take before it begins, decided purely so the precedence is pinned:
 * a missing mic permission is reported ahead of a missing model (no point naming the model when
 * idiolect can't even hear), and only an all-clear lets the take start.
 */
class RecognitionPreconditionsTest {
    @Test
    fun all_clear_lets_the_take_start() {
        assertNull(RecognitionPreconditions.blocker(hasMicPermission = true, hasModel = true))
    }

    @Test
    fun a_missing_mic_permission_is_reported_first() {
        assertEquals(
            RecognitionError.MIC_PERMISSION,
            RecognitionPreconditions.blocker(hasMicPermission = false, hasModel = false),
        )
    }

    @Test
    fun a_missing_model_blocks_when_the_mic_is_granted() {
        assertEquals(
            RecognitionError.MODEL_MISSING,
            RecognitionPreconditions.blocker(hasMicPermission = true, hasModel = false),
        )
    }
}
