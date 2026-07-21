package org.idiolect.android.recognition

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Bundle
import android.speech.RecognitionService
import androidx.annotation.VisibleForTesting
import androidx.core.content.ContextCompat
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
    @VisibleForTesting
    internal var take: RecognitionTake? = null

    override fun onStartListening(recognizerIntent: Intent, listener: Callback) {
        // Unlike the activity, this headless path cannot prompt for a runtime permission, so a
        // missing mic must be reported to the caller as ERROR_INSUFFICIENT_PERMISSIONS (rather
        // than letting the AudioRecord path fail later as a generic recognition error). Mic is
        // checked ahead of the model — see RecognitionPreconditions.
        startBlocker()?.let {
            reportError(listener, it)
            return
        }
        val model = ModelStore(File(filesDir, "models/whisper")).active()
            ?: run {
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

    /**
     * What blocks a take here before any native core is loaded: no `RECORD_AUDIO` (mic first,
     * since the service can't prompt) or no installed model — the ordering is the unit-tested
     * [RecognitionPreconditions]. Pulled out so the Android permission/model read is covered by a
     * Robolectric test; the `Callback` error wiring itself is covered by the connected e2e.
     */
    @VisibleForTesting
    internal fun startBlocker(): RecognitionError? {
        val model = ModelStore(File(filesDir, "models/whisper")).active()
        return RecognitionPreconditions.blocker(hasMicPermission(), hasModel = model != null)
    }

    private fun hasMicPermission(): Boolean =
        ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    private fun reportError(listener: Callback, error: RecognitionError) {
        runCatching { listener.error(RecognitionErrorCodes.speechRecognizer(error)) }
        cleanup()
    }

    private fun cleanup() {
        runCatching { MicForegroundService.stop(this) }
        // Cancel BEFORE release: a take still listening when the service is destroyed (no
        // onCancel from the framework) would otherwise keep capturing with no owner —
        // release() alone does not stop the mic. cancel() is a no-op on a spent session.
        take?.cancel()
        take?.release()
        take = null
    }

    override fun onDestroy() {
        cleanup()
        super.onDestroy()
    }
}
