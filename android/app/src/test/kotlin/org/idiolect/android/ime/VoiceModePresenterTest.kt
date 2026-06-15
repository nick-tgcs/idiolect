package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The voice-mode view's status logic: idle → listening while recording → back to idle,
 * with a decode error surfaced only *after* the take ends (it would be noise mid-take)
 * and cleared when the next take starts.
 */
class VoiceModePresenterTest {
    @Test
    fun idle_by_default() {
        assertEquals(VoiceStatus.Idle, VoiceModePresenter().status())
    }

    @Test
    fun recording_shows_listening() {
        val presenter = VoiceModePresenter()
        assertEquals(VoiceStatus.Listening, presenter.onRecordingChanged(true))
    }

    @Test
    fun stopping_a_clean_take_returns_to_idle() {
        val presenter = VoiceModePresenter()
        presenter.onRecordingChanged(true)
        assertEquals(VoiceStatus.Idle, presenter.onRecordingChanged(false))
    }

    @Test
    fun an_error_during_a_take_is_masked_then_surfaces_when_it_stops() {
        val presenter = VoiceModePresenter()
        presenter.onRecordingChanged(true)
        // Masked while still listening — don't distract mid-take.
        assertEquals(VoiceStatus.Listening, presenter.onError("no model"))
        assertEquals(VoiceStatus.Error("no model"), presenter.onRecordingChanged(false))
    }

    @Test
    fun a_new_take_clears_a_prior_error() {
        val presenter = VoiceModePresenter()
        presenter.onRecordingChanged(true)
        presenter.onError("no model")
        presenter.onRecordingChanged(false)
        assertEquals(VoiceStatus.Listening, presenter.onRecordingChanged(true))
    }
}
