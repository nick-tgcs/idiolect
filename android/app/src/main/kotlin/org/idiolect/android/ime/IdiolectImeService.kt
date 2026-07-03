package org.idiolect.android.ime

import android.Manifest
import android.annotation.SuppressLint
import android.content.pm.PackageManager
import android.inputmethodservice.InputMethodService
import android.os.Handler
import android.os.Looper
import android.view.Gravity
import android.view.MotionEvent
import android.view.View
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.FrameLayout
import android.widget.HorizontalScrollView
import android.widget.ImageButton
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
import androidx.core.content.ContextCompat
import org.idiolect.android.R
import org.idiolect.android.audio.AndroidPcmSource
import org.idiolect.android.audio.MicForegroundService
import org.idiolect.android.core.CoreCallbackRouter
import org.idiolect.android.core.IdiolectCoreHost
import org.idiolect.android.model.ModelStore
import org.idiolect.android.settings.SettingsActivity
import org.idiolect.android.settings.SettingsStore
import org.idiolect.android.sync.SyncScheduler
import org.idiolect.ffi.IdiolectCore
import java.io.File
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread

/**
 * The idiolect Android IME. Owns the on-device core for the life of the service, drives
 * one-tap dictation through [MicToggle]/[DictationController], and renders the core's
 * push callbacks into the focused field via [IdiolectImeCallback].
 *
 * **One surface: the mic.** idiolect never renders its own typing keyboard — to edit, it
 * hands the field to the user's *own* keyboard via the system IME switch (the `⌨` button,
 * see [SwitchToYourKeyboard]). The `👁` toggle is review mode: a finished take opens the
 * centred [ReviewActivity] (edited with the user's keyboard, the edit captured as a training
 * pair) before it lands. So the view is just a status line + a circular mic + a control strip
 * (⌨ switch-keyboard · 👁 review · ⚙ settings).
 *
 * Voice status comes from [VoiceModePresenter]; recording state is the core's to decide,
 * so the view waits for [ImeUiHost.onRecordingChanged] rather than flipping optimistically
 * (the daemon's single-source-of-truth model).
 */
class IdiolectImeService : InputMethodService(), ImeUiHost, KeyboardHandoff {
    private lateinit var core: IdiolectCore
    private lateinit var router: CoreCallbackRouter
    private lateinit var imeCallback: IdiolectImeCallback
    private lateinit var mic: MicToggle
    private lateinit var controller: DictationController
    private lateinit var correction: CorrectionCapture
    // The always-available editing keys (⌫ backspace, ⏎ enter) on the control strip.
    private val keyActions = KeyActions(::fieldEditor)
    private val presenter = VoiceModePresenter()
    private val main = Handler(Looper.getMainLooper())
    // Mic taps run here, off the UI thread: the finalize toggle re-transcribes the whole
    // take (seconds of whisper). One thread keeps taps ordered; shut down in onDestroy.
    private val toggleExecutor = Executors.newSingleThreadExecutor { r -> Thread(r, "idiolect-mic-toggle") }
    private val modelStore by lazy { ModelStore(File(filesDir, "models/whisper")) }
    // Persisted dictation/sync toggles set on the settings screen (⚙). Read here so the strip's
    // 👁 default, the double-tap-for-continuous gesture, and outbox shipping honour the user's choice.
    private val settingsStore by lazy { SettingsStore.under(filesDir) }
    private val modelLoadStarted = AtomicBoolean(false)
    private var root: FrameLayout? = null
    private var statusView: TextView? = null
    private var micButton: ImageButton? = null
    private var progressBar: ProgressBar? = null
    private var reviewButton: ImageButton? = null
    // The in-surface review card that shows the live transcript while a review-mode take runs
    // (review on ⇒ words never touch the host field — they stream here). GONE when empty.
    private var liveCard: TextView? = null
    // 👁 review mode: OFF = the take lands in the field directly; ON = a finished take opens
    // the centred review surface first. Toggled on the UI thread, read on the core's callback
    // thread (commitText) → @Volatile.
    @Volatile
    private var reviewEnabled = false
    // Whether the live take is continuous: review is suppressed for it (reviewing every phrase
    // would be absurd). Set on the UI thread, read on the callback thread → @Volatile.
    @Volatile
    private var takeIsContinuous = false
    // Set once the core is torn down in onDestroy; guards late core calls (a final
    // onFinishInput / a queued capture) from crashing on a destroyed core.
    @Volatile
    private var coreClosed = false

    override fun onCreate() {
        super.onCreate()
        // The core lives in a process-wide host so it survives this service being torn down
        // on a keyboard switch (review captures corrections while another keyboard is active).
        // Route the core's pushes to THIS ime while it's the active keyboard.
        val host = IdiolectCoreHost.acquire(this)
        core = host.core
        router = host.router
        imeCallback = IdiolectImeCallback(editorProvider = ::fieldEditor, ui = this)
        router.bind(imeCallback)
        controller = DictationController(
            sink = { frame -> core.pushPcmFrame(frame) },
            sourceFactory = { AndroidPcmSource() },
        )
        mic = MicToggle(core = CoreRecordingToggle(core), capture = controller, executor = toggleExecutor)
        correction = CorrectionCapture(
            editor = ::fieldEditor,
            // Guard against the core being torn down: the framework can fire a final
            // onFinishInput (→ capture) during teardown, and a queued main-thread capture can
            // land after close(). Calling a destroyed core throws — drop the late correction.
            reportCorrection = { if (!coreClosed) core.reportCorrection(it) },
            // Editing a chip's word means typing — which idiolect doesn't do. Select the
            // word, then hand the field to the user's own keyboard to type over it; the
            // edit is captured when idiolect regains focus (onStartInputView).
            onEnterEdit = { switchToYourKeyboard() },
        )
        // Start the 👁 toggle from the persisted default (set on the settings screen). A tiny
        // flag-file read; the toggle is read later on the callback thread, so seed it now.
        reviewEnabled = settingsStore.reviewByDefault()
    }

    /**
     * Nudge the outbox toward the PC (M6). A no-op if nothing is pending or no endpoint is
     * configured; WorkManager defers it until there's a network, and the scheduler's KEEP
     * policy collapses a burst of corrections into one job.
     *
     * Gated by the "Ship corrections to your PC" setting (⚙): when off, corrections are still
     * captured locally — just not shipped, so they accumulate in the outbox until it's re-enabled.
     */
    private fun scheduleSync() {
        if (settingsStore.shipCorrections()) SyncScheduler.enqueue(this)
    }

    /** The live field as a [FieldEditor], or `null` between fields. */
    private fun fieldEditor(): FieldEditor? =
        currentInputConnection?.let { InputConnectionFieldEditor(it) }

    override fun onStartInputView(info: EditorInfo?, restarting: Boolean) {
        super.onStartInputView(info, restarting)
        // idiolect was summoned for its OWN review dialog's edit field — it has no keyboard,
        // so hand off to the user's real keyboard and show nothing here.
        if (info?.privateImeOptions == ReviewActivity.REVIEW_FIELD_OPTION) {
            switchToYourKeyboard()
            return
        }
        // Lazy model init on first focus (plan §1.2): load the installed model off the
        // main thread, verifying its SHA-256 (M5a) before use. Until it loads, a take
        // finalizes to nothing and the user is told a model is needed.
        ensureModelLoaded()
        // The review just approved some text — type it now that we're back on a real field
        // (the deferred-insert tail of the 👁 flow; capture already happened in the Activity).
        PendingInsert.take()?.let { fieldEditor()?.commitText(it) }
        // Returning here after editing in the user's keyboard: read the field back and, if
        // the last take changed, record the raw→corrected pair. A no-op otherwise.
        if (correction.capture()) scheduleSync()
    }

    private fun ensureModelLoaded() {
        if (!modelLoadStarted.compareAndSet(false, true)) return
        val model = modelStore.active()
        if (model == null) {
            modelLoadStarted.set(false) // retry on a later focus once one is installed
            return
        }
        thread(isDaemon = true, name = "idiolect-model-load") {
            runCatching { core.loadModelVerified(model.path, model.sha256) }
                .onFailure { error -> onDictationError("couldn't load the model: ${error.message}") }
        }
    }

    override fun onCreateInputView(): View {
        val container = FrameLayout(this)
        root = container
        renderVoiceSurface()
        // Render live partials onto whatever live card is currently built (it's rebuilt on each
        // surface render, so the listener reads the field rather than capturing a stale view).
        // push() arrives on the core's callback thread → marshal to main.
        LiveReview.bind { text -> main.post { showLiveCard(text) } }
        return container
    }

    /** Show/hide the in-surface review card with the current live transcript. */
    private fun showLiveCard(text: String) {
        liveCard?.apply {
            this.text = text
            visibility = if (text.isEmpty()) View.GONE else View.VISIBLE
        }
    }

    /** (Re)build the one-and-only surface — the mic view — in place (no new window). */
    private fun renderVoiceSurface() {
        val container = root ?: return
        container.removeAllViews()
        // Drop the old view's references first; a stray status push mid-rebuild is then a
        // safe no-op (render() guards on null).
        statusView = null
        micButton = null
        progressBar = null
        reviewButton = null
        liveCard = null
        container.addView(buildVoiceView())
    }

    private fun buildVoiceView(): View {
        val status = TextView(this).apply {
            textSize = 13f
            gravity = Gravity.CENTER
        }
        statusView = status

        // Thin accent progress bar shown above the mic only while transcribing.
        val progress = ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal).apply {
            isIndeterminate = true
            indeterminateTintList =
                ContextCompat.getColorStateList(this@IdiolectImeService, R.color.idiolect_accent)
            visibility = View.INVISIBLE
        }
        progressBar = progress

        // One big circular mic; its disc colour (set in render) is the state feedback.
        // Touch (not click) so the recogniser can tell hold / tap / double-tap apart.
        @SuppressLint("ClickableViewAccessibility")
        val micKey = ImageButton(this).apply {
            setImageResource(R.drawable.ic_mic)
            scaleType = ImageView.ScaleType.CENTER_INSIDE
            contentDescription = getString(R.string.voice_mic_desc)
            setOnTouchListener { view, event ->
                when (event.actionMasked) {
                    MotionEvent.ACTION_DOWN -> recognizer.onDown()
                    MotionEvent.ACTION_UP -> {
                        view.performClick() // accessibility: still announce a click
                        recognizer.onUp()
                    }
                    MotionEvent.ACTION_CANCEL -> recognizer.onCancel()
                    else -> return@setOnTouchListener false
                }
                true
            }
        }
        micButton = micKey

        // The live review card: while review mode is on, the streaming transcript shows here
        // (never in the host field). Styled like the review dialog's field; hidden when empty.
        val live = TextView(this).apply {
            textSize = 15f
            gravity = Gravity.TOP or Gravity.START
            minLines = 2
            background = ContextCompat.getDrawable(this@IdiolectImeService, R.drawable.review_field_bg)
            setPadding(dp(12), dp(11), dp(12), dp(11))
            setTextColor(ContextCompat.getColor(this@IdiolectImeService, R.color.idiolect_text))
            visibility = View.GONE
        }
        liveCard = live

        render(presenter.status())
        val stage = FrameLayout(this).apply {
            addView(micKey, FrameLayout.LayoutParams(dp(92), dp(92), Gravity.CENTER))
        }

        return LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
            setBackgroundColor(ContextCompat.getColor(this@IdiolectImeService, R.color.idiolect_panel))
            setPadding(dp(14), dp(10), dp(14), dp(16))
            addView(
                buildControlStrip(),
                LinearLayout.LayoutParams(LinearLayout.LayoutParams.MATCH_PARENT, dp(46)),
            )
            buildCorrectionStrip()?.let { addView(it, wrap()) }
            addView(
                progress,
                LinearLayout.LayoutParams(dp(120), dp(3)).apply { topMargin = dp(11) },
            )
            addView(status, wrap())
            addView(
                live,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.MATCH_PARENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                ).apply { topMargin = dp(8) },
            )
            addView(
                stage,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                ).apply { topMargin = dp(8) },
            )
        }
    }

    /** The rounded control-strip pill: ⌫ backspace · ⌨ switch-to-your-keyboard · 👁 review-before-insert · ⚙ settings · ⏎ enter. */
    private fun buildControlStrip(): View {
        val row = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setBackgroundResource(R.drawable.strip_pill)
        }
        // ⌫ and ⏎ bookend the strip: the common edits reached for right after a take —
        // fix the tail, or send/newline — always available whether or not a take is live.
        row.addView(
            stripButton(R.drawable.ic_backspace, R.string.voice_strip_backspace, lit = false) { onBackspaceKey() },
            stripLp(),
        )
        row.addView(
            stripButton(R.drawable.ic_keyboard, R.string.voice_strip_keyboard, lit = false) { switchToYourKeyboard() },
            stripLp(),
        )
        val review = stripButton(
            R.drawable.ic_review,
            R.string.voice_strip_review,
            lit = reviewEnabled,
        ) { toggleReview() }
        reviewButton = review
        row.addView(review, stripLp())
        row.addView(
            stripButton(R.drawable.ic_settings, R.string.voice_strip_settings, lit = false) { openSettings() },
            stripLp(),
        )
        row.addView(
            stripButton(R.drawable.ic_enter, R.string.voice_strip_enter, lit = false) { onEnterKey() },
            stripLp(),
        )
        return row
    }

    private fun stripLp() = LinearLayout.LayoutParams(0, dp(36), 1f).apply {
        leftMargin = dp(6); rightMargin = dp(6); topMargin = dp(5); bottomMargin = dp(5)
    }

    private fun stripButton(iconRes: Int, descRes: Int, lit: Boolean, onClick: () -> Unit): ImageButton =
        ImageButton(this).apply {
            setImageResource(iconRes)
            scaleType = ImageView.ScaleType.CENTER_INSIDE
            contentDescription = getString(descRes)
            paintStripButton(this, lit)
            setOnClickListener { onClick() }
        }

    /** Paint a strip button lit (accent pill, white glyph) or idle (transparent, grey glyph). */
    private fun paintStripButton(button: ImageButton, lit: Boolean) {
        button.background =
            if (lit) ContextCompat.getDrawable(this, R.drawable.strip_ib_on) else null
        button.imageTintList = ContextCompat.getColorStateList(
            this,
            if (lit) R.color.mic_glyph_active else R.color.idiolect_grey,
        )
    }

    /** Flip the 👁 review toggle: when lit, a finished take opens the centred review surface
     * (edited with the user's keyboard, captured as a training pair) before it lands. */
    private fun toggleReview() {
        reviewEnabled = !reviewEnabled
        reviewButton?.let { paintStripButton(it, reviewEnabled) }
        // The 👁 strip toggle and the settings "Review before insert" switch are one setting:
        // persist the flip so it sticks across service teardowns (keyboard switches) and shows
        // through on the settings screen. Off the UI thread — it's a file write.
        val persist = reviewEnabled
        thread(isDaemon = true, name = "idiolect-pref-review") { settingsStore.setReviewByDefault(persist) }
    }

    /** Open the settings screen (⚙ on the strip): pairing, dictation modes, model, storage. */
    private fun openSettings() = SettingsActivity.launch(this)

    /** ⌫ key: delete the character before the cursor in the focused field. */
    private fun onBackspaceKey() = keyActions.backspace()

    /** ⏎ key: perform the field's editor action (Send/Search/Go/Done) or, when it declares
     * none, insert a newline — the same behaviour as the system Enter key for that field. */
    private fun onEnterKey() = keyActions.enter(currentInputEditorInfo?.imeOptions ?: 0)

    private fun dp(value: Int): Int = (value * resources.displayMetrics.density).toInt()

    /**
     * The post-take correction strip: the committed words as tappable chips. Tapping a
     * chip selects that word's range and hands the field to the user's own keyboard so the
     * next keystroke replaces it (plan §1.4). `null` when there is no committed take yet.
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

    /** Schedules the gesture timers on the main thread (the [MicGestureRecognizer]'s clock). */
    private val gestureClock = object : GestureClock {
        private val tokens = HashMap<Any, Runnable>()
        override fun postDelayed(delayMs: Long, token: Any, action: () -> Unit) {
            remove(token)
            val runnable = Runnable {
                tokens.remove(token)
                action()
            }
            tokens[token] = runnable
            main.postDelayed(runnable, delayMs)
        }
        override fun remove(token: Any) {
            tokens.remove(token)?.let { main.removeCallbacks(it) }
        }
    }

    /** Maps recognised gestures onto the mic + presenter (see [MicGestureRecognizer]). */
    private val micGestures = object : MicGestures {
        // Press-to-talk: show the red Holding look at once (before the core confirms recording),
        // then open the mic. A single tap deliberately does NOT — it stays the accent Listening.
        override fun onHoldStart() {
            render(presenter.onHoldStarted())
            startTake()
        }
        override fun onHoldEnd() = stopTake()
        override fun onSingleTap() = if (isRecordingUi()) stopTake() else startTake()
        override fun onDoubleTap() = when {
            isRecordingUi() -> stopTake()
            // "Continuous on double-tap" (⚙) off ⇒ a double-tap is just a one-shot take.
            settingsStore.continuousOnDoubleTap() -> startContinuous()
            else -> startTake()
        }
    }

    private val recognizer = MicGestureRecognizer(micGestures, gestureClock)

    /** Whether the UI currently shows a live take — read from the presenter, never the core
     * (which would block the UI thread behind a decode). */
    private fun isRecordingUi(): Boolean =
        presenter.status().let {
            it is VoiceStatus.Listening || it is VoiceStatus.Holding || it is VoiceStatus.Continuous
        }

    /** Start a one-shot take (a hold, or a tap from idle). */
    private fun startTake() {
        if (!hasMicPermission()) {
            onDictationError(getString(R.string.idiolect_mic_permission_required))
            return
        }
        takeIsContinuous = false
        mic.startHold()
    }

    /** Stop the live take with instant "Transcribing…" feedback while the decode runs. */
    private fun stopTake() {
        render(presenter.onStopRequested())
        mic.stop()
    }

    /** Enter continuous mode (a double-tap from idle): show it at once, then open the mic. */
    private fun startContinuous() {
        if (!hasMicPermission()) {
            onDictationError(getString(R.string.idiolect_mic_permission_required))
            return
        }
        takeIsContinuous = true
        render(presenter.onContinuousStarted())
        mic.startContinuous()
    }

    /**
     * Hand the field to the user's **own** keyboard — idiolect has no keyboard of its own.
     * Prefers their last-used IME, then the next, then the system picker ([SwitchToYourKeyboard]).
     * They return to idiolect via their keyboard's switch key / the system IME switcher
     * (Android forbids an app from force-selecting a different IME).
     */
    private fun switchToYourKeyboard() = SwitchToYourKeyboard.run(this)

    // --- KeyboardHandoff: the thin system-IME-switch boundary behind [SwitchToYourKeyboard]. ---

    override fun toPreviousKeyboard(): Boolean = switchToPreviousInputMethod()

    override fun toNextKeyboard(): Boolean {
        // `switchToNextInputMethod` only rotates IMEs already in the switch *history*, which
        // is empty when idiolect was selected from Settings (the common case) — so it returns
        // false despite other keyboards being enabled. Fall back to switching to a specific
        // enabled keyboard by id, which is deterministic (the system picker is unreliable
        // from an IME).
        if (switchToNextInputMethod(false)) return true
        val imm = getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager
        val target = KeyboardTargets.pickOther(
            imm.enabledInputMethodList.map { EnabledKeyboard(it.id, it.packageName) },
            ownPackage = packageName,
        ) ?: return false
        return runCatching {
            switchInputMethod(target)
            true
        }.getOrDefault(false)
    }

    override fun openPicker() {
        (getSystemService(INPUT_METHOD_SERVICE) as InputMethodManager).showInputMethodPicker()
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
        if (!recording) takeIsContinuous = false
        val status = presenter.onRecordingChanged(recording)
        main.post { render(status) }
    }

    override fun onCommit(text: String) {
        // Seed the correction strip from the committed take (fires under the core lock, so
        // the strip render is marshalled to the main thread).
        correction.onTakeCommitted(text)
        main.post { renderVoiceSurface() } // refresh so the new chips appear
    }

    override fun isReviewEnabled(): Boolean = reviewEnabled && !takeIsContinuous

    override fun onLivePreedit(text: String) {
        // Review mode: the live partials stream onto idiolect's own review card via the channel
        // (never the host field). Fires on the core's callback thread; the bound listener marshals
        // the render to the main thread.
        LiveReview.push(text)
    }

    override fun onReviewRequested(text: String) {
        // A finished take, with review on. The take is already persisted with a history id;
        // open the centred review surface for it and DON'T type it into the host field. This
        // fires on the core's callback thread under its lock.
        // Clear any live preview from the host field — the take goes through review.
        main.post {
            fieldEditor()?.apply {
                setComposingText("")
                finishComposingText()
            }
            // Hide the in-surface live card; the seeded review dialog now takes over.
            LiveReview.reset()
        }
        // The history lookup decrypts the on-disk store — do it off the callback/main thread
        // (on main it stalled the dialog open by ~hundreds of ms), then launch from main. A
        // background thread also can't deadlock on the core lock: this callback has already
        // returned and released it by the time the lookup runs.
        thread(isDaemon = true, name = "idiolect-review-launch") {
            val id = runCatching { core.recentHistory(1u).firstOrNull()?.id }.getOrNull()
            main.post {
                if (id != null) {
                    ReviewActivity.launch(this, id, text)
                } else {
                    // No history id (shouldn't happen) — type it so nothing is lost.
                    fieldEditor()?.commitText(text)
                }
            }
        }
    }

    override fun onFinishInput() {
        // Field going away: capture any pending in-field correction before we lose it,
        // then nudge the outbox so the fresh learning ships when there's a network.
        correction.capture()
        scheduleSync()
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
        // super.onDestroy() fires a final onFinishInput → correction.capture() → the core,
        // so it MUST run while the core is still alive. Tear the core down only afterwards.
        // (Lifecycle ordering isn't unit-testable without Robolectric — this module runs pure
        // JVM unit tests + UI-Automator e2e — so it's verified on-device.)
        super.onDestroy()
        controller.stop()
        toggleExecutor.shutdown()
        LiveReview.bind(null) // stop streaming live partials to this dying surface
        router.unbind(imeCallback) // stop routing core pushes to this dying IME
        coreClosed = true
        // Drop our reference. The core only closes if nothing else holds it — the review
        // Activity keeps it alive across this teardown so it can capture the correction.
        IdiolectCoreHost.release()
    }

    private fun render(status: VoiceStatus) {
        val visual = VoiceVisuals.forStatus(status)
        val (statusText, statusColor) = when (status) {
            is VoiceStatus.Idle ->
                getString(R.string.voice_idle_hint) to R.color.idiolect_muted
            is VoiceStatus.Listening ->
                getString(R.string.voice_listening) to R.color.idiolect_accent_bright
            is VoiceStatus.Holding ->
                getString(R.string.voice_holding) to R.color.idiolect_live
            is VoiceStatus.Continuous ->
                getString(R.string.voice_continuous) to R.color.idiolect_live
            is VoiceStatus.Transcribing ->
                getString(R.string.voice_transcribing) to R.color.idiolect_grey
            is VoiceStatus.Error ->
                status.message to R.color.idiolect_live
        }
        micButton?.apply {
            setBackgroundResource(visual.backgroundRes)
            imageTintList = ContextCompat.getColorStateList(this@IdiolectImeService, visual.glyphTintRes)
        }
        progressBar?.visibility = if (visual.showProgress) View.VISIBLE else View.INVISIBLE
        statusView?.apply {
            text = statusText
            setTextColor(ContextCompat.getColor(this@IdiolectImeService, statusColor))
        }
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
        override fun startContinuous() {
            core.startContinuous()
        }
    }
}
