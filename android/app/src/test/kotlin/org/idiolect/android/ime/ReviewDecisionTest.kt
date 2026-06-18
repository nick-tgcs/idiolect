package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The pure decisions behind the review flow (👁): whether a finished take is reviewed
 * before it lands, whether the user's edit is a real correction worth recording as
 * training data, and what (if anything) gets typed into the field on Insert. Capturing
 * the correction is the whole point, so these are unit-pinned.
 */
class ReviewDecisionTest {
    @Test
    fun a_one_shot_take_is_reviewed_when_review_is_on() {
        assertTrue(ReviewDecision.shouldReview(reviewEnabled = true, continuous = false))
    }

    @Test
    fun nothing_is_reviewed_when_review_is_off() {
        assertFalse(ReviewDecision.shouldReview(reviewEnabled = false, continuous = false))
    }

    @Test
    fun a_continuous_take_is_never_reviewed_per_phrase() {
        assertFalse(ReviewDecision.shouldReview(reviewEnabled = true, continuous = true))
    }

    @Test
    fun an_edit_that_changes_the_text_is_a_correction() {
        assertTrue(ReviewDecision.isCorrection(raw = "pick up the kids", edited = "pick up the kid"))
    }

    @Test
    fun inserting_the_text_unchanged_is_not_a_correction() {
        // Reviewed and accepted as-is — there is no raw→corrected pair to record.
        assertFalse(ReviewDecision.isCorrection(raw = "hello there", edited = "hello there"))
        // Whitespace-only differences don't count either.
        assertFalse(ReviewDecision.isCorrection(raw = "hello there", edited = "  hello there  "))
    }

    @Test
    fun a_blank_edit_is_not_a_correction() {
        // The user cleared it — don't poison training with an empty "correction".
        assertFalse(ReviewDecision.isCorrection(raw = "hello", edited = "   "))
    }

    @Test
    fun the_text_to_insert_is_the_edit_unless_blank() {
        assertEquals("fixed text", ReviewDecision.textToInsert("fixed text"))
        assertNull(ReviewDecision.textToInsert("   "))
    }
}
