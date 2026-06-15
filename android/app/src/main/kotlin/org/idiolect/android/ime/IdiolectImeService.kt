package org.idiolect.android.ime

import android.Manifest
import android.content.pm.PackageManager
import android.inputmethodservice.InputMethodService
import android.view.Gravity
import android.view.View
import android.widget.Button
import android.widget.FrameLayout
import androidx.core.content.ContextCompat
import org.idiolect.android.R
import org.idiolect.android.audio.AndroidPcmSource
import org.idiolect.ffi.IdiolectCore

/**
 * The idiolect Android IME. Owns the on-device core for the life of the service, drives
 * one-tap dictation through [MicToggle]/[DictationController], and renders the core's
 * push callbacks into the focused field via [IdiolectImeCallback].
 *
 * Recording state is the core's to decide: this service waits for
 * [ImeUiHost.onRecordingChanged] to paint the mic indicator rather than flipping it
 * optimistically (the daemon's single-source-of-truth model).
 */
class IdiolectImeService : InputMethodService(), ImeUiHost {
    private lateinit var core: IdiolectCore
    private lateinit var mic: MicToggle
    private lateinit var controller: DictationController
    private var micButton: Button? = null

    override fun onCreate() {
        super.onCreate()
        val callback = IdiolectImeCallback(
            editorProvider = {
                currentInputConnection?.let { InputConnectionFieldEditor(it) }
            },
            ui = this,
        )
        core = IdiolectCore(filesDir.absolutePath, callback)
        controller = DictationController(
            sink = { frame -> core.pushPcmFrame(frame) },
            sourceFactory = { AndroidPcmSource() },
        )
        mic = MicToggle(core = CoreRecordingToggle(core), capture = controller)
    }

    override fun onCreateInputView(): View {
        val button = Button(this).apply {
            text = idleLabel()
            setOnClickListener { onMicTapped() }
        }
        micButton = button
        return FrameLayout(this).apply {
            addView(
                button,
                FrameLayout.LayoutParams(
                    FrameLayout.LayoutParams.WRAP_CONTENT,
                    FrameLayout.LayoutParams.WRAP_CONTENT,
                ).apply { gravity = Gravity.CENTER },
            )
        }
    }

    private fun onMicTapped() {
        // Privacy gate: never open the mic without the runtime permission.
        if (!hasMicPermission()) {
            onDictationError(getString(R.string.idiolect_mic_permission_required))
            return
        }
        mic.onTap()
    }

    private fun hasMicPermission(): Boolean =
        ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    // --- ImeUiHost: the core's non-typing pushes ---

    override fun onRecordingChanged(recording: Boolean) {
        micButton?.text = if (recording) recordingLabel() else idleLabel()
    }

    override fun onEditHistory(id: Long, text: String) {
        // The review dialog lands in a later increment (M4).
    }

    override fun onDictationError(message: String) {
        // A proper status line lands in a later increment; never crash on a failed take.
    }

    override fun onDestroy() {
        controller.stop()
        core.close()
        super.onDestroy()
    }

    private fun idleLabel() = getString(R.string.idiolect_mic_idle)

    private fun recordingLabel() = getString(R.string.idiolect_mic_recording)

    /** Adapts [IdiolectCore] to the [RecordingToggle] the mic key drives. */
    private class CoreRecordingToggle(private val core: IdiolectCore) : RecordingToggle {
        override fun isRecording(): Boolean = core.isRecording()
        override fun toggle() {
            core.toggle()
        }
    }
}
