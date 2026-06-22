package org.idiolect.android.ime

/** What the voice-mode view shows. */
sealed interface VoiceStatus {
    data object Idle : VoiceStatus
    data object Listening : VoiceStatus

    /**
     * Press-and-hold (press-to-talk) is recording: the mic shows the red "release to send"
     * look. Distinct from [Listening] (a single-tap take) so the red is specific to a hold,
     * set the instant the hold gesture is recognised and held until the hold is released.
     */
    data object Holding : VoiceStatus

    /**
     * The take was stopped and the whole-take decode is running (off the UI thread).
     * Shown the instant the user stops, so the mic gives immediate feedback rather than
     * looking stuck on "Listening…" until the seconds-long decode commits.
     */
    data object Transcribing : VoiceStatus

    /**
     * Continuous dictation (entered by double-tapping the mic): the mic stays open and
     * each phrase types as the speaker pauses, until a single tap stops it. Distinct from
     * [Listening] so the mic can show the "● Continuous — tap to stop" look.
     */
    data object Continuous : VoiceStatus
    data class Error(val message: String) : VoiceStatus
}

/**
 * Reduces the core's recording-state and error pushes into the voice-mode view's
 * [VoiceStatus]. An error raised during a take is held back until the take fully stops
 * (it would be noise mid-listen or mid-decode) and cleared when the next take begins.
 * Synchronized because errors arrive on the pump thread and recording changes on the
 * main thread.
 */
class VoiceModePresenter {
    private var recording = false

    /**
     * Set the moment the user stops a take (UI thread), before the core's
     * `recording_status(false)` arrives — the whole-take decode is still running. Takes
     * precedence over [recording] so the view flips to [VoiceStatus.Transcribing] at once;
     * cleared when the finalize completes (recording stops) or the next take begins.
     */
    private var transcribing = false

    /**
     * Set the moment a continuous take is requested (the mic's double-tap), before the
     * core's `recording_status(true)` arrives. It is the user's *mode* intent (the
     * recording state itself still comes from the core); cleared when recording stops.
     */
    private var continuous = false

    /**
     * Set the moment a press-and-hold is recognised (the mic's hold gesture), before the
     * core's `recording_status(true)` arrives — so the mic flips to the red [VoiceStatus.Holding]
     * look at once. Cleared when recording stops or a continuous take is requested.
     */
    private var holding = false
    private var error: String? = null

    @Synchronized
    fun onRecordingChanged(recording: Boolean): VoiceStatus {
        this.recording = recording
        // A new take clears a prior error; recording_status(false) is pushed last — after
        // decode + commit — so by the time either edge arrives the transcribe is over.
        if (recording) error = null
        transcribing = false
        // Recording stopped → the continuous/hold take is over (don't let the red stick).
        if (!recording) {
            continuous = false
            holding = false
        }
        return status()
    }

    /** The user stopped the take: show transcribing immediately while the decode runs. */
    @Synchronized
    fun onStopRequested(): VoiceStatus {
        transcribing = true
        return status()
    }

    /** The user pressed-and-held the mic: show the red press-to-talk look at once. */
    @Synchronized
    fun onHoldStarted(): VoiceStatus {
        holding = true
        error = null
        transcribing = false
        return status()
    }

    /** The user double-tapped to enter continuous mode: show it at once. */
    @Synchronized
    fun onContinuousStarted(): VoiceStatus {
        continuous = true
        // A hold can't also be continuous — the explicit continuous intent supersedes it.
        holding = false
        error = null
        transcribing = false
        return status()
    }

    @Synchronized
    fun onError(message: String): VoiceStatus {
        error = message
        return status()
    }

    @Synchronized
    fun status(): VoiceStatus {
        val currentError = error
        return when {
            transcribing -> VoiceStatus.Transcribing
            continuous -> VoiceStatus.Continuous
            holding -> VoiceStatus.Holding
            recording -> VoiceStatus.Listening
            currentError != null -> VoiceStatus.Error(currentError)
            else -> VoiceStatus.Idle
        }
    }
}
