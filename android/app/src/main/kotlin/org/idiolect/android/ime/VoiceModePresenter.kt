package org.idiolect.android.ime

/** What the voice-mode view shows. */
sealed interface VoiceStatus {
    data object Idle : VoiceStatus
    data object Listening : VoiceStatus

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
    private var error: String? = null

    @Synchronized
    fun onRecordingChanged(recording: Boolean): VoiceStatus {
        this.recording = recording
        // A new take clears a prior error; recording_status(false) is pushed last — after
        // decode + commit — so by the time either edge arrives the transcribe is over.
        if (recording) error = null
        transcribing = false
        // Recording stopped → the continuous take is over.
        if (!recording) continuous = false
        return status()
    }

    /** The user stopped the take: show transcribing immediately while the decode runs. */
    @Synchronized
    fun onStopRequested(): VoiceStatus {
        transcribing = true
        return status()
    }

    /** The user double-tapped to enter continuous mode: show it at once. */
    @Synchronized
    fun onContinuousStarted(): VoiceStatus {
        continuous = true
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
            recording -> VoiceStatus.Listening
            currentError != null -> VoiceStatus.Error(currentError)
            else -> VoiceStatus.Idle
        }
    }
}
