package org.idiolect.android.ime

/** What the voice-mode view shows. */
sealed interface VoiceStatus {
    data object Idle : VoiceStatus
    data object Listening : VoiceStatus
    data class Error(val message: String) : VoiceStatus
}

/**
 * Reduces the core's recording-state and error pushes into the voice-mode view's
 * [VoiceStatus]. An error raised during a take is held back until the take stops (it
 * would be noise mid-listen) and cleared when the next take begins. Synchronized
 * because errors arrive on the pump thread and recording changes on the main thread.
 */
class VoiceModePresenter {
    private var recording = false
    private var error: String? = null

    @Synchronized
    fun onRecordingChanged(recording: Boolean): VoiceStatus {
        this.recording = recording
        if (recording) error = null
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
            recording -> VoiceStatus.Listening
            currentError != null -> VoiceStatus.Error(currentError)
            else -> VoiceStatus.Idle
        }
    }
}
