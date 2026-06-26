package org.idiolect.android.recognition

import android.content.Intent
import android.os.Bundle
import android.speech.RecognitionService
import org.idiolect.android.audio.MicForegroundService
import org.idiolect.android.model.ModelStore
import java.io.File

/**
 * idiolect as a **system speech engine**: a [RecognitionService] so idiolect is selectable in the
 * device's "voice input / speech recognition" setting and usable by any app through the
 * `SpeechRecognizer` API. It shares the on-device whisper engine ([CoreRecognitionTake]) with the
 * `ACTION_RECOGNIZE_SPEECH` activity, mapping the take's outcome onto the framework [Callback].
 *
 * Unlike the activity this runs with no UI, so a take needs the microphone foreground service to
 * capture in the background — started best-effort (a background FGS start can be refused on newer
 * Android; the take still records while the caller is foreground). Registration is guarded by
 * [VoiceProviderManifestTest]; the once-only/blank logic is the unit-tested [RecognitionSession].
 */
class IdiolectRecognitionService : RecognitionService() {
    private var take: CoreRecognitionTake? = null

    override fun onStartListening(recognizerIntent: Intent, listener: Callback) {
        val model = ModelStore(File(filesDir, "models/whisper")).active()
        if (model == null) {
            reportError(listener, RecognitionError.MODEL_MISSING)
            return
        }
        runCatching { MicForegroundService.start(this) }
        val live = CoreRecognitionTake(this)
        take = live
        live.begin(
            model,
            object : RecognitionOutput {
                override fun onReadyForSpeech() {
                    runCatching {
                        listener.readyForSpeech(Bundle())
                        listener.beginningOfSpeech()
                    }
                }

                override fun onResult(text: String) {
                    runCatching { listener.results(RecognitionResults.bundle(text)) }
                    cleanup()
                }

                override fun onError(error: RecognitionError) {
                    reportError(listener, error)
                }
            },
        )
    }

    override fun onStopListening(listener: Callback) {
        runCatching { listener.endOfSpeech() }
        take?.stopListening()
    }

    override fun onCancel(listener: Callback) {
        take?.cancel()
        cleanup()
    }

    private fun reportError(listener: Callback, error: RecognitionError) {
        runCatching { listener.error(RecognitionErrorCodes.speechRecognizer(error)) }
        cleanup()
    }

    private fun cleanup() {
        runCatching { MicForegroundService.stop(this) }
        take?.release()
        take = null
    }

    override fun onDestroy() {
        cleanup()
        super.onDestroy()
    }
}
