package org.idiolect.android.audio

import android.annotation.SuppressLint
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder

/**
 * [PcmSource] backed by `AudioRecord` at exactly 16 kHz mono PCM-16 — the geometry the
 * core's pipeline expects, so there is no resampler on device (unlike the desktop).
 * The `VOICE_RECOGNITION` source applies the platform's speech-tuned signal path.
 *
 * Thin framework seam: covered by the emulator e2e, not host unit tests (`AudioRecord`
 * has no meaningful headless behaviour). The capture loop driving it is unit-tested
 * via the [PcmSource] interface ([org.idiolect.android.ime.DictationControllerTest]).
 */
class AndroidPcmSource : PcmSource {
    private var record: AudioRecord? = null

    // RECORD_AUDIO is checked and gated by the IME before a take starts.
    @SuppressLint("MissingPermission")
    override fun start() {
        val minBuffer = AudioRecord.getMinBufferSize(SAMPLE_RATE_HZ, CHANNEL, ENCODING)
        // At least a second of headroom so a brief stall never overruns the mic buffer.
        val bufferBytes = maxOf(minBuffer, SAMPLE_RATE_HZ * BYTES_PER_SAMPLE)
        val recorder = AudioRecord(
            MediaRecorder.AudioSource.VOICE_RECOGNITION,
            SAMPLE_RATE_HZ,
            CHANNEL,
            ENCODING,
            bufferBytes,
        )
        // If the recorder couldn't initialise (no usable mic — happens on emulators),
        // release it and fail clearly rather than calling startRecording() on a dead object.
        // AudioCapture catches this and ends the take cleanly instead of crashing.
        if (recorder.state != AudioRecord.STATE_INITIALIZED) {
            recorder.release()
            throw IllegalStateException("AudioRecord failed to initialise (state=${recorder.state})")
        }
        recorder.startRecording()
        record = recorder
    }

    override fun read(into: ShortArray): Int {
        val recorder = record ?: return 0
        val read = recorder.read(into, 0, into.size)
        // A negative result is an error/stopped state; report 0 so the loop ends.
        return if (read < 0) 0 else read
    }

    override fun stop() {
        record?.let { recorder ->
            // AudioRecord.stop() throws IllegalStateException if the recorder errored or was
            // never initialised; never let that escape (it runs on both the capture thread
            // and the main thread via DictationController.stop()). Always release the native
            // buffer regardless.
            runCatching { recorder.stop() }
            runCatching { recorder.release() }
        }
        record = null
    }

    private companion object {
        const val SAMPLE_RATE_HZ = 16_000
        const val CHANNEL = AudioFormat.CHANNEL_IN_MONO
        const val ENCODING = AudioFormat.ENCODING_PCM_16BIT
        const val BYTES_PER_SAMPLE = 2
    }
}
