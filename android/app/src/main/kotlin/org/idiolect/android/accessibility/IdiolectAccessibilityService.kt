package org.idiolect.android.accessibility

import android.Manifest
import android.accessibilityservice.AccessibilityService
import android.content.pm.PackageManager
import android.os.Bundle
import android.provider.Settings
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
import android.view.inputmethod.InputMethodManager
import org.idiolect.android.ime.EnabledKeyboard
import org.idiolect.android.ime.ImeReturn
import org.idiolect.android.ime.ImeSelection
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

    override fun onInterrupt() = Unit

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

    companion object {
        /** The shared file the dialog writes and this service drains (see [InjectQueue]). */
        const val PENDING_FILE = "pending_insert"
    }
}
