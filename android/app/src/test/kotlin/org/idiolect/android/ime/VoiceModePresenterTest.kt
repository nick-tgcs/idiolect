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

    // --- Transcribing: the instant-feedback state shown the moment a take is stopped,
    // while the (multi-second) whole-take decode runs off the UI thread, until the core
    // confirms the finalize is done via recording_status(false). ---

    @Test
    fun stopping_a_take_immediately_shows_transcribing_even_before_recording_clears() {
        val presenter = VoiceModePresenter()
        presenter.onRecordingChanged(true)
        // The UI thread requests stop; the core hasn't pushed recording_status(false) yet
        // (the decode is still running), but the user must see feedback at once.
        assertEquals(VoiceStatus.Transcribing, presenter.onStopRequested())
    }

    @Test
    fun transcribing_clears_to_idle_when_the_finalize_completes() {
        val presenter = VoiceModePresenter()
        presenter.onRecordingChanged(true)
        presenter.onStopRequested()
        // recording_status(false) is pushed last, after decode+commit → done.
        assertEquals(VoiceStatus.Idle, presenter.onRecordingChanged(false))
    }

    @Test
    fun transcribing_masks_an_error_until_the_finalize_completes() {
        val presenter = VoiceModePresenter()
        presenter.onRecordingChanged(true)
        presenter.onStopRequested()
        assertEquals(VoiceStatus.Transcribing, presenter.onError("decode failed"))
        assertEquals(VoiceStatus.Error("decode failed"), presenter.onRecordingChanged(false))
    }

    @Test
    fun a_new_take_clears_a_pending_transcribing_state() {
        val presenter = VoiceModePresenter()
        presenter.onRecordingChanged(true)
        presenter.onStopRequested()
        assertEquals(VoiceStatus.Listening, presenter.onRecordingChanged(true))
    }

    // --- Continuous: double-tap enters continuous mode (mic stays open, each phrase
    // types as you pause). Shown the instant the gesture is recognised, then held across
    // the core's recording confirmation, until a stop closes it. ---

    @Test
    fun double_tap_shows_continuous_immediately() {
        val presenter = VoiceModePresenter()
        assertEquals(VoiceStatus.Continuous, presenter.onContinuousStarted())
    }

    @Test
    fun continuous_survives_the_cores_recording_confirmation() {
        val presenter = VoiceModePresenter()
        presenter.onContinuousStarted()
        // start_continuous pushes recording_status(true); it must NOT downgrade to Listening.
        assertEquals(VoiceStatus.Continuous, presenter.onRecordingChanged(true))
    }

    @Test
    fun continuous_clears_to_idle_when_recording_stops() {
        val presenter = VoiceModePresenter()
        presenter.onContinuousStarted()
        presenter.onRecordingChanged(true)
        assertEquals(VoiceStatus.Idle, presenter.onRecordingChanged(false))
    }

    @Test
    fun stopping_continuous_shows_transcribing_first() {
        val presenter = VoiceModePresenter()
        presenter.onContinuousStarted()
        presenter.onRecordingChanged(true)
        // The user taps once to stop: the final phrase is decoding.
        assertEquals(VoiceStatus.Transcribing, presenter.onStopRequested())
        assertEquals(VoiceStatus.Idle, presenter.onRecordingChanged(false))
    }

    @Test
    fun a_plain_take_is_listening_not_continuous() {
        val presenter = VoiceModePresenter()
        assertEquals(VoiceStatus.Listening, presenter.onRecordingChanged(true))
    }
}
