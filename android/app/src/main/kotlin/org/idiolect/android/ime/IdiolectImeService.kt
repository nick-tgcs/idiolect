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
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.LinearLayout
import android.widget.TextView
import androidx.core.content.ContextCompat
import org.idiolect.android.R
import org.idiolect.android.audio.AndroidPcmSource
import org.idiolect.android.audio.MicForegroundService
import org.idiolect.android.crypto.HistoryKey
import org.idiolect.ffi.IdiolectCore
import java.io.File

/**
 * The idiolect Android IME. Owns the on-device core for the life of the service, drives
 * one-tap dictation through [MicToggle]/[DictationController], and renders the core's
 * push callbacks into the focused field via [IdiolectImeCallback].
 *
 * One surface, two modes ([ModePresenter]) that swap in place: the **voice** view is a
 * status line + a mic key + an `⌨` flip to edit + a `🌐` system handoff; the **edit**
 * view is a tap-only QWERTY ([KeyboardLayout]/[EditKeyboard]) whose `🎤` flips back. The
 * toggle is symmetric and always one tap.
 *
 * Voice status comes from [VoiceModePresenter]; recording state is the core's to decide,
 * so the view waits for [ImeUiHost.onRecordingChanged] rather than flipping optimistically
 * (the daemon's single-source-of-truth model).
 */
class IdiolectImeService : InputMethodService(), ImeUiHost {
    private lateinit var core: IdiolectCore
    private lateinit var mic: MicToggle
    private lateinit var controller: DictationController
    private lateinit var editKeyboard: EditKeyboard
    private lateinit var correction: CorrectionCapture
    private val presenter = VoiceModePresenter()
    private val mode = ModePresenter()
    private val main = Handler(Looper.getMainLooper())
    private var root: FrameLayout? = null
    private var statusView: TextView? = null
    private var micButton: Button? = null

    override fun onCreate() {
        super.onCreate()
        val callback = IdiolectImeCallback(editorProvider = ::fieldEditor, ui = this)
        // At-rest encryption of the history projection: a 32-byte key wrapped by the
        // hardware-backed AndroidKeyStore, generated once under filesDir.
        val historyKey = HistoryKey.load(File(filesDir, HistoryKey.FILE_NAME))
        core = IdiolectCore(filesDir.absolutePath, historyKey, callback)
        controller = DictationController(
            sink = { frame -> core.pushPcmFrame(frame) },
            sourceFactory = { AndroidPcmSource() },
        )
        mic = MicToggle(core = CoreRecordingToggle(core), capture = controller)
        correction = CorrectionCapture(
            editor = ::fieldEditor,
            reportCorrection = { core.reportCorrection(it) },
            onEnterEdit = { showMode(KeyboardMode.Edit) },
        )
        editKeyboard = EditKeyboard(
            editor = ::fieldEditor,
            // Leaving edit mode = done correcting: read the field back and record the
            // raw→corrected pair before returning to voice.
            onSwitchToVoice = { captureCorrectionThenVoice() },
        )
    }

    /** Capture any in-field correction, then return to voice mode (showing fresh chips). */
    private fun captureCorrectionThenVoice() {
        correction.capture()
        showMode(KeyboardMode.Voice)
    }

    /** The live field as a [FieldEditor], or `null` between fields. */
    private fun fieldEditor(): FieldEditor? =
        currentInputConnection?.let { InputConnectionFieldEditor(it) }

    override fun onCreateInputView(): View {
        val container = FrameLayout(this)
        root = container
        showMode(mode.current())
        return container
    }

    /** Swap the input view to [target] in place (no new window). */
    private fun showMode(target: KeyboardMode) {
        mode.show(target)
        val container = root ?: return
        container.removeAllViews()
        // The voice view owns these references; clear them so a stray status push while
        // in edit mode is a safe no-op (render() guards on null).
        statusView = null
        micButton = null
        container.addView(
            when (target) {
                KeyboardMode.Voice -> buildVoiceView()
                KeyboardMode.Edit -> buildEditView()
            },
        )
    }

    /** Stop any live take, then flip to edit mode (can't dictate and edit at once). */
    private fun enterEditMode() {
        if (core.isRecording()) mic.onTap()
        showMode(KeyboardMode.Edit)
    }

    private fun buildVoiceView(): View {
        val pad = (16 * resources.displayMetrics.density).toInt()
        val status = TextView(this).apply { textSize = 16f }
        val micKey = Button(this).apply { setOnClickListener { onMicTapped() } }
        val editKey = Button(this).apply {
            text = getString(R.string.voice_edit_mode)
            setOnClickListener { enterEditMode() }
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
            buildCorrectionStrip()?.let { addView(it, wrap()) }
            addView(status, wrap())
            addView(micKey, wrap())
            addView(editKey, wrap())
            addView(switchKey, wrap())
        }
    }

    /**
     * The post-take correction strip: the committed words as tappable chips. Tapping a
     * chip selects that word's range and flips to edit mode so the next keystroke
     * replaces it (plan §1.4). `null` when there is no committed take yet.
     */
    private fun buildCorrectionStrip(): View? {
        val chips = correction.currentChips()
        if (chips.isEmpty()) return null
        val row = LinearLayout(this).apply { orientation = LinearLayout.HORIZONTAL }
        chips.forEachIndexed { index, chip ->
            row.addView(
                Button(this).apply {
                    text = chip.text
                    setOnClickListener { correction.selectWord(index) }
                },
            )
        }
        return HorizontalScrollView(this).apply { addView(row) }
    }

    /** The tap-only QWERTY (state in [editKeyboard]; this is the declared GUI seam). */
    private fun buildEditView(): View {
        val charButtons = mutableListOf<Pair<Button, Key.Character>>()
        val keyboard = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        KeyboardLayout.QWERTY.forEach { keyRow ->
            val rowView = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_HORIZONTAL
            }
            keyRow.forEach { key ->
                val button = Button(this).apply {
                    text = keyLabel(key)
                    setOnClickListener {
                        editKeyboard.onKey(key)
                        applyShift(charButtons)
                    }
                }
                if (key is Key.Character) charButtons.add(button to key)
                rowView.addView(button)
            }
            keyboard.addView(rowView)
        }
        applyShift(charButtons)
        return keyboard
    }

    /** Repaint letter caps to match the one-shot shift state. */
    private fun applyShift(charButtons: List<Pair<Button, Key.Character>>) {
        charButtons.forEach { (button, key) ->
            button.text = if (editKeyboard.isShifted) key.upper else key.lower
        }
    }

    private fun keyLabel(key: Key): String = when (key) {
        is Key.Character -> key.lower
        Key.Shift -> "⇧"
        Key.Backspace -> "⌫"
        Key.Space -> "space"
        Key.Enter -> "⏎"
        Key.SwitchToVoice -> "🎤"
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

    override fun onCommit(text: String) {
        // Seed the correction strip from the committed take (fires under the core lock,
        // so the strip render is marshalled to the main thread). Refresh the voice view
        // if it's showing, so the new chips appear.
        correction.onTakeCommitted(text)
        main.post { if (mode.current() == KeyboardMode.Voice) showMode(KeyboardMode.Voice) }
    }

    override fun onFinishInput() {
        // Field going away: capture any pending in-field correction before we lose it.
        correction.capture()
        super.onFinishInput()
    }

    override fun onEditHistory(id: Long, text: String) {
        // The review/history screen is the companion Activity's job (plan §1.6), not the
        // IME input view — nothing to do here.
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
