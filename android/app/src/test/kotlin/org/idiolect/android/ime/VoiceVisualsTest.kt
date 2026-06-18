package org.idiolect.android.ime

import org.idiolect.android.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The pure state→look mapping for the circular mic. Keeping the resource choice in a
 * testable function (rather than inline in the View) lets us assert each [VoiceStatus]
 * gets a distinct, correct background/glyph/progress without touching the Android view
 * layer — the View that *applies* the spec is the (untestable) GUI seam.
 */
class VoiceVisualsTest {
    @Test
    fun idle_is_the_slate_mic_with_a_purple_glyph_and_no_progress() {
        val v = VoiceVisuals.forStatus(VoiceStatus.Idle)
        assertEquals(R.drawable.mic_idle, v.backgroundRes)
        assertEquals(R.color.mic_glyph_idle, v.glyphTintRes)
        assertFalse(v.showProgress)
    }

    @Test
    fun listening_is_the_accent_mic_with_a_white_glyph() {
        val v = VoiceVisuals.forStatus(VoiceStatus.Listening)
        assertEquals(R.drawable.mic_listening, v.backgroundRes)
        assertEquals(R.color.mic_glyph_active, v.glyphTintRes)
        assertFalse(v.showProgress)
    }

    @Test
    fun continuous_is_the_live_red_mic_with_a_white_glyph() {
        val v = VoiceVisuals.forStatus(VoiceStatus.Continuous)
        assertEquals(R.drawable.mic_continuous, v.backgroundRes)
        assertEquals(R.color.mic_glyph_active, v.glyphTintRes)
        assertFalse(v.showProgress)
    }

    @Test
    fun transcribing_is_grey_and_shows_the_progress_bar() {
        val v = VoiceVisuals.forStatus(VoiceStatus.Transcribing)
        assertEquals(R.drawable.mic_transcribing, v.backgroundRes)
        assertTrue(v.showProgress)
    }

    @Test
    fun error_falls_back_to_the_idle_look_with_no_progress() {
        val v = VoiceVisuals.forStatus(VoiceStatus.Error("no model"))
        assertEquals(R.drawable.mic_error, v.backgroundRes)
        assertFalse(v.showProgress)
    }

    @Test
    fun the_states_each_have_a_visually_distinct_background() {
        val backgrounds = listOf(
            VoiceStatus.Idle,
            VoiceStatus.Listening,
            VoiceStatus.Continuous,
            VoiceStatus.Transcribing,
            VoiceStatus.Error("x"),
        ).map { VoiceVisuals.forStatus(it).backgroundRes }
        assertEquals("each state must look different", backgrounds.size, backgrounds.toSet().size)
        // Sanity: listening (the live state) is never confused with idle.
        assertNotEquals(
            VoiceVisuals.forStatus(VoiceStatus.Idle).backgroundRes,
            VoiceVisuals.forStatus(VoiceStatus.Listening).backgroundRes,
        )
    }
}
