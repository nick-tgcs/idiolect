package org.idiolect.android.accessibility

import android.Manifest
import android.accessibilityservice.AccessibilityButtonController
import android.accessibilityservice.AccessibilityService
import android.content.pm.PackageManager
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.inputmethod.InputMethodManager
import android.widget.Toast
import androidx.annotation.VisibleForTesting
import org.idiolect.android.R
import org.idiolect.android.ime.EnabledKeyboard
import org.idiolect.android.ime.ImeReturn
import org.idiolect.android.ime.ImeSelection
import org.idiolect.android.model.ModelStore
import org.idiolect.android.recognition.CoreRecognitionTake
import org.idiolect.android.recognition.RecognitionError
import org.idiolect.android.recognition.RecognitionOutput
import org.idiolect.android.recognition.RecognitionPreconditions
import org.idiolect.android.recognition.RecognitionTake
import org.idiolect.android.settings.SettingsStore
import java.io.File

/**
 * The one Android API that lets idiolect type the reviewed correction straight into the user's
 * app **while their own keyboard is the active IME** — so the 👁 review dialog can edit with a
 * real keyboard and still land the result automatically (no manual switch back to idiolect).
 *
 * The dialog stashes the approved text in a file-backed [InjectQueue]; this service drains it
 * the moment the user's field regains focus after the dialog closes (a focus or window-state
 * event), splicing the text in at the field's cursor ([TextInjection]) via `ACTION_SET_TEXT`.
 * Going through a file — rather than calling the service in-process — is deliberate: the dialog
 * and this service can be bound in different processes, and the file crosses that boundary.
 *
 * It only ever injects into an editable field in *another* app ([InjectionTargeting]), never
 * idiolect's own review card, and reads nothing else. The node calls have no headless seam
 * (real [AccessibilityNodeInfo]s exist only on a device), so the wiring is covered by the
 * connected e2e; the splice, the targeting rule, and the queue are unit-tested.
 */
class IdiolectAccessibilityService : AccessibilityService() {
    private val queue: InjectQueue by lazy { InjectQueue(File(filesDir, PENDING_FILE)) }

    /** Marshals recognition callbacks (core/load threads) back to the main thread for node work. */
    private val main = Handler(Looper.getMainLooper())

    /** A live quick-launch take, or null when idle. Non-null ⇒ a take is listening/transcribing. */
    @VisibleForTesting
    internal var quickTake: RecognitionTake? = null

    private val quickLaunchButton = object : AccessibilityButtonController.AccessibilityButtonCallback() {
        override fun onClicked(controller: AccessibilityButtonController) = onQuickLaunchButton()
    }

    override fun onInterrupt() = Unit

    override fun onServiceConnected() {
        super.onServiceConnected()
        // Make Android's floating accessibility button — when the user has pointed it at idiolect —
        // start dictation instead of doing nothing (the reported "quick-launch does nothing" bug).
        // Registering the callback routes its taps to onClicked; flagRequestAccessibilityButton in
        // accessibility_service.xml makes idiolect an eligible target.
        runCatching { accessibilityButtonController.registerAccessibilityButtonCallback(quickLaunchButton) }
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) {
        event ?: return
        when (event.eventType) {
            AccessibilityEvent.TYPE_VIEW_FOCUSED,
            AccessibilityEvent.TYPE_VIEW_TEXT_SELECTION_CHANGED,
            AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED,
            -> drainInto(focusedHostField())
        }
    }

    /** Inject the pending correction into [target] if there is one and a field to take it. */
    private fun drainInto(target: AccessibilityNodeInfo?) {
        val pending = queue.take() ?: return
        if (target == null || !inject(target, pending)) {
            // No field yet (or the set failed) — keep it queued for the next focus event.
            queue.put(pending)
            return
        }
        // The reviewed text just landed in the host field — pull the active keyboard back to
        // idiolect's mic so the user can dictate again without a manual keyboard switch.
        // This MUST happen here, not from the review dialog: the dialog handed the field to the
        // user's own keyboard (which persistently makes *that* the default IME), and writing the
        // setting back while the review field is still focused only makes idiolect re-bind it and
        // hand back off — the default bounces straight back and the mic never returns. By this
        // focus event the dialog is gone and a host field has focus, so the rewrite sticks.
        restoreIdiolectIme()
    }

    /**
     * Re-select idiolect as the default IME so the mic returns after a reviewed Insert. Android
     * forbids an app from selecting an IME without [Manifest.permission.WRITE_SECURE_SETTINGS] (a
     * deliberate anti-hijack rule) — a one-time `adb pm grant`; without it this is a silent no-op
     * and the user returns to idiolect via the system IME switcher. [ImeReturn] skips the write
     * when idiolect is already active.
     */
    private fun restoreIdiolectIme() {
        if (checkSelfPermission(Manifest.permission.WRITE_SECURE_SETTINGS) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val imm = getSystemService(InputMethodManager::class.java) ?: return
        // Use the framework's OWN id for idiolect's IME (its short flattenToShortString form),
        // taken straight from the enabled list — a reconstructed long-form id is rejected by
        // InputMethodManagerService as "Unknown id" and the switch silently fails (mic never
        // returns). Null means idiolect isn't an enabled IME, so there's nothing to switch to.
        val enabled = imm.enabledInputMethodList.map { EnabledKeyboard(it.id, it.packageName) }
        val idiolect = ImeSelection.idiolectImeId(enabled, packageName) ?: return
        val current = Settings.Secure.getString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
        if (!ImeReturn.shouldRestore(current, idiolect)) return
        runCatching {
            Settings.Secure.putString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD, idiolect)
        }
    }

    /** The input-focused editable field in the foreground app, or null if that isn't one. */
    private fun focusedHostField(): AccessibilityNodeInfo? {
        val root = rootInActiveWindow ?: return null
        val focused = root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT) ?: return null
        return focused.takeIf {
            InjectionTargeting.isHostField(it.packageName?.toString(), packageName, it.isEditable)
        }
    }

    /** Splice [text] into [node] at its cursor and drop the caret after it. */
    private fun inject(node: AccessibilityNodeInfo, text: String): Boolean {
        // An empty field reports its *hint* as getText() (e.g. "Search…"), so treat a
        // hint-showing field as empty — otherwise we'd splice onto the placeholder.
        val showingHint = node.isShowingHintText
        val existing = if (showingHint) "" else node.text?.toString().orEmpty()
        val spliced = TextInjection.spliceAtSelection(
            existing,
            if (showingHint) -1 else node.textSelectionStart,
            if (showingHint) -1 else node.textSelectionEnd,
            text,
        )
        val set = node.performAction(
            AccessibilityNodeInfo.ACTION_SET_TEXT,
            Bundle().apply {
                putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, spliced.text)
            },
        )
        if (!set) return false
        // Best-effort caret placement; a field that rejects selection still got the text.
        node.performAction(
            AccessibilityNodeInfo.ACTION_SET_SELECTION,
            Bundle().apply {
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, spliced.cursor)
                putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, spliced.cursor)
            },
        )
        return true
    }

    // --- Quick-launch: the floating accessibility button → dictate into the focused field -------

    /**
     * A tap on Android's floating accessibility button. Honours the in-app toggle, then tap-to-start
     * / tap-to-stop a take. The transcript lands in whatever field is focused via the same [inject]
     * path the review flow uses — so quick-launch works in any app without switching the keyboard.
     */
    private fun onQuickLaunchButton() {
        val enabled = SettingsStore.under(filesDir).quickLaunchEnabled()
        when (QuickLaunch.decide(enabled = enabled, recording = quickTake != null)) {
            QuickLaunchAction.Disabled -> toast(getString(R.string.quicklaunch_disabled))
            QuickLaunchAction.Stop -> {
                toast(getString(R.string.quicklaunch_transcribing))
                quickTake?.stopListening()
            }
            QuickLaunchAction.Start -> startQuickLaunch()
        }
    }

    /** Begin a take (mic + model permitting); the transcript arrives at [onQuickResult]. */
    private fun startQuickLaunch() {
        val model = ModelStore(File(filesDir, "models/whisper")).active()
        val blocker = RecognitionPreconditions.blocker(hasMicPermission(), model != null)
        if (blocker != null) {
            val message = if (blocker == RecognitionError.MIC_PERMISSION) {
                R.string.quicklaunch_mic_needed
            } else {
                R.string.recognize_no_model
            }
            toast(getString(message))
            return
        }
        if (model == null) return // unreachable once blocker is null; satisfies null-safety
        val take = CoreRecognitionTake(this)
        quickTake = take
        toast(getString(R.string.quicklaunch_listening))
        take.begin(
            model,
            object : RecognitionOutput {
                override fun onReadyForSpeech() = Unit
                override fun onResult(text: String) {
                    main.post { onQuickResult(text) }
                }

                override fun onError(error: RecognitionError) {
                    main.post { onQuickError(error) }
                }
            },
        )
    }

    /** Splice the finished transcript into the focused field; nudge the user if there isn't one. */
    private fun onQuickResult(text: String) {
        val field = focusedHostField()
        val landed = field != null && inject(field, text)
        if (!landed) toast(getString(R.string.quicklaunch_no_field))
        finishQuickTake()
    }

    private fun onQuickError(error: RecognitionError) {
        val message = when (error) {
            RecognitionError.NO_SPEECH -> R.string.recognize_no_speech
            RecognitionError.MIC_PERMISSION -> R.string.quicklaunch_mic_needed
            RecognitionError.MODEL_MISSING -> R.string.recognize_no_model
            RecognitionError.FAILED -> R.string.recognize_failed
        }
        toast(getString(message))
        finishQuickTake()
    }

    private fun finishQuickTake() {
        // Cancel BEFORE release: release() drops the core reference but does not stop a
        // still-listening capture, so a take live at destroy time would keep the mic running
        // with no owner. cancel() is a no-op once a result/error already spent the session.
        quickTake?.cancel()
        quickTake?.release()
        quickTake = null
    }

    private fun hasMicPermission(): Boolean =
        checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    private fun toast(message: String) {
        Toast.makeText(this, message, Toast.LENGTH_SHORT).show()
    }

    override fun onDestroy() {
        finishQuickTake()
        super.onDestroy()
    }

    companion object {
        /** The shared file the dialog writes and this service drains (see [InjectQueue]). */
        const val PENDING_FILE = "pending_insert"
    }
}
