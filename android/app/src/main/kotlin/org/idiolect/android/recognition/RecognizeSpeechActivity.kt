package org.idiolect.android.recognition

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Bundle
import android.speech.RecognizerIntent
import android.view.Gravity
import android.view.View
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.TextView
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import org.idiolect.android.R
import org.idiolect.android.model.ModelStore
import java.io.File

/**
 * idiolect's answer to `ACTION_RECOGNIZE_SPEECH` — the voice picker an app's built-in mic button
 * opens (`startActivityForResult`). This is the list the user found idiolect **missing** from
 * (only Google and a sibling app appeared). It shows a minimal "Listening…" surface, records
 * on-device through the whisper core ([CoreRecognitionTake]), and returns the transcript as
 * `EXTRA_RESULTS` so the host app pastes it.
 *
 * No headless seam (real mic + core + the activity-result plumbing), so registration is guarded by
 * [VoiceProviderManifestTest] and the flow is covered by the connected e2e; the once-only/blank
 * *logic* is the unit-tested [RecognitionSession].
 */
class RecognizeSpeechActivity : ComponentActivity() {
    private var take: CoreRecognitionTake? = null
    private var status: TextView? = null

    private val askMic =
        registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
            if (granted) startRecognition() else finishCancelled()
        }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContentView(buildSurface())
        if (hasMicPermission()) startRecognition() else askMic.launch(Manifest.permission.RECORD_AUDIO)
    }

    private fun startRecognition() {
        val model = ModelStore(File(filesDir, "models/whisper")).active()
        if (model == null) {
            // Keep the surface up with the reason rather than flashing and vanishing — the user
            // can read why nothing happened, then back out (which returns CANCELED).
            status?.setText(R.string.recognize_no_model)
            return
        }
        val live = CoreRecognitionTake(this)
        take = live
        live.begin(
            model,
            object : RecognitionOutput {
                override fun onReadyForSpeech() = runOnUiThread { status?.setText(R.string.recognize_listening) }
                override fun onResult(text: String) = runOnUiThread { returnTranscript(text) }
                override fun onError(error: RecognitionError) = runOnUiThread { reportError(error) }
            },
        )
    }

    /** Tap anywhere = "I'm done": finalize the take; the transcript returns via [onResult]. A tap
     *  while still "Preparing…" (model loading) ends the take as no-speech instead — the mic must
     *  never open after the user already asked to stop. */
    private fun onSurfaceTapped() {
        if (take == null) return
        status?.setText(R.string.recognize_transcribing)
        take?.stopListening()
    }

    private fun returnTranscript(text: String) {
        setResult(
            Activity.RESULT_OK,
            Intent().putStringArrayListExtra(RecognizerIntent.EXTRA_RESULTS, RecognitionResults.list(text)),
        )
        finish()
    }

    private fun reportError(error: RecognitionError) {
        toast(
            getString(
                when (error) {
                    RecognitionError.NO_SPEECH -> R.string.recognize_no_speech
                    RecognitionError.MIC_PERMISSION -> R.string.recognize_mic_needed
                    RecognitionError.MODEL_MISSING -> R.string.recognize_no_model
                    RecognitionError.FAILED -> R.string.recognize_failed
                },
            ),
        )
        finishCancelled()
    }

    private fun finishCancelled() {
        setResult(Activity.RESULT_CANCELED)
        finish()
    }

    override fun onDestroy() {
        // Back-press / a returned result both land here: suppress any late transcript and drop the
        // core reference. cancel() is a no-op once a result already finalized the session.
        take?.cancel()
        take?.release()
        take = null
        super.onDestroy()
    }

    private fun buildSurface(): View {
        val mic = ImageView(this).apply {
            setImageResource(R.drawable.ic_mic)
            setColorFilter(ContextCompat.getColor(this@RecognizeSpeechActivity, R.color.mic_glyph_active))
            contentDescription = MIC_DESC
            layoutParams = LinearLayout.LayoutParams(dp(72), dp(72))
        }
        status = TextView(this).apply {
            setText(R.string.recognize_preparing)
            setTextColor(ContextCompat.getColor(this@RecognizeSpeechActivity, R.color.idiolect_text))
            textSize = 16f
            gravity = Gravity.CENTER
        }
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER
            // A dim scrim over the host app (the activity theme is translucent) so the focus is
            // the mic, while the app the text will land in stays visible behind it.
            setBackgroundColor(Color.parseColor("#e605060a"))
            setPadding(dp(32), dp(32), dp(32), dp(32))
            addView(mic)
            addView(
                status,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                ).apply { topMargin = dp(20) },
            )
            setOnClickListener { onSurfaceTapped() }
        }
    }

    private fun hasMicPermission(): Boolean =
        ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    private fun toast(message: String) =
        android.widget.Toast.makeText(this, message, android.widget.Toast.LENGTH_SHORT).show()

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    companion object {
        /** Content description on the mic glyph — the stable handle the connected e2e looks for. */
        const val MIC_DESC = "idiolect voice input"
    }
}
