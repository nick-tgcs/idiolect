package org.idiolect.android.ime

import android.Manifest
import android.content.pm.PackageManager
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.View
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.content.ContextCompat
import org.idiolect.android.R
import org.idiolect.android.audio.AndroidPcmSource
import org.idiolect.android.audio.MicForegroundService
import org.idiolect.ffi.IdiolectCore

/**
 * The idiolect Android IME. Owns the on-device core for the life of the service, drives
 * one-tap dictation through [MicToggle]/[DictationController], and renders the core's
 * push callbacks into the focused field via [IdiolectImeCallback].
 *
 * The voice-mode view is a status line + a mic key + a keyboard-switch handoff. Its
 * status comes from [VoiceModePresenter]; recording state is the core's to decide, so
 * the view waits for [ImeUiHost.onRecordingChanged] rather than flipping optimistically
 * (the daemon's single-source-of-truth model).
 */
class IdiolectImeService : InputMethodService(), ImeUiHost {
    private lateinit var core: IdiolectCore
    private lateinit var mic: MicToggle
    private lateinit var controller: DictationController
    private val presenter = VoiceModePresenter()
    private val main = Handler(Looper.getMainLooper())
    private var statusView: TextView? = null
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
        val pad = (16 * resources.displayMetrics.density).toInt()
        val status = TextView(this).apply { textSize = 16f }
        val micKey = Button(this).apply {
            setOnClickListener { onMicTapped() }
        }
        val switchKey = Button(this).apply {
            text = getString(R.string.voice_switch_keyboard)
            setOnClickListener { switchAwayFromIme() }
        }
        statusView = status
        micButton = micKey
        render(presenter.status())
        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setPadding(pad, pad, pad, pad)
            addView(status, wrap())
            addView(micKey, wrap())
            addView(switchKey, wrap())
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

    /** The `🌐` handoff: switch to the user's other keyboard (e.g. for editing). */
    private fun switchAwayFromIme() {
        if (!switchToNextInputMethod(false)) {
            (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).showInputMethodPicker()
        }
    }

    private fun hasMicPermission(): Boolean =
        ContextCompat.checkSelfPermission(this, Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED

    // --- ImeUiHost: the core's non-typing pushes ---

    override fun onRecordingChanged(recording: Boolean) {
        // Hold the mic foreground service exactly while recording. This fires inside
        // the core's toggle (before capture starts / after it stops), so the FGS is up
        // before AudioRecord and down after it — and follows the single source of truth.
        if (recording) MicForegroundService.start(this) else MicForegroundService.stop(this)
        val status = presenter.onRecordingChanged(recording)
        main.post { render(status) }
    }

    override fun onEditHistory(id: Long, text: String) {
        // The review dialog lands in a later increment (M4).
    }

    override fun onDictationError(message: String) {
        // Surface in the status line (held back until the take stops); never crash.
        val status = presenter.onError(message)
        main.post { render(status) }
    }

    override fun onDestroy() {
        controller.stop()
        core.close()
        super.onDestroy()
    }

    private fun render(status: VoiceStatus) {
        val (label, statusText) = when (status) {
            is VoiceStatus.Idle -> getString(R.string.idiolect_mic_idle) to ""
            is VoiceStatus.Listening ->
                getString(R.string.idiolect_mic_recording) to getString(R.string.voice_listening)
            is VoiceStatus.Error ->
                getString(R.string.idiolect_mic_idle) to status.message
        }
        micButton?.text = label
        statusView?.text = statusText
    }

    private fun wrap() = LinearLayout.LayoutParams(
        LinearLayout.LayoutParams.WRAP_CONTENT,
        LinearLayout.LayoutParams.WRAP_CONTENT,
    ).apply { gravity = Gravity.CENTER_HORIZONTAL; topMargin = 12 }

    /** Adapts [IdiolectCore] to the [RecordingToggle] the mic key drives. */
    private class CoreRecordingToggle(private val core: IdiolectCore) : RecordingToggle {
        override fun isRecording(): Boolean = core.isRecording()
        override fun toggle() {
            core.toggle()
        }
    }
}
