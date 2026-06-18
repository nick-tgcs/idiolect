package org.idiolect.android.ime

import androidx.annotation.ColorRes
import androidx.annotation.DrawableRes
import org.idiolect.android.R

/** The look of the circular mic for a given [VoiceStatus]: which disc, glyph tint, progress. */
data class MicVisual(
    @DrawableRes val backgroundRes: Int,
    @ColorRes val glyphTintRes: Int,
    /** Show the thin progress bar above the mic (the multi-second decode is running). */
    val showProgress: Boolean,
)

/**
 * Pure state→look mapping for the voice mic, kept out of the View so it's unit-testable.
 * The colour change *is* the feedback: idle slate w/ purple glyph → accent disc while
 * listening → grey disc + progress while transcribing → slate ringed red on error.
 */
object VoiceVisuals {
    fun forStatus(status: VoiceStatus): MicVisual = when (status) {
        VoiceStatus.Idle ->
            MicVisual(R.drawable.mic_idle, R.color.mic_glyph_idle, showProgress = false)
        VoiceStatus.Listening ->
            MicVisual(R.drawable.mic_listening, R.color.mic_glyph_active, showProgress = false)
        VoiceStatus.Continuous ->
            MicVisual(R.drawable.mic_continuous, R.color.mic_glyph_active, showProgress = false)
        VoiceStatus.Transcribing ->
            MicVisual(R.drawable.mic_transcribing, R.color.mic_glyph_idle, showProgress = true)
        is VoiceStatus.Error ->
            MicVisual(R.drawable.mic_error, R.color.mic_glyph_idle, showProgress = false)
    }
}
