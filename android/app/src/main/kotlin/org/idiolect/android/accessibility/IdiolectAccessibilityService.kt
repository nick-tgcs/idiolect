package org.idiolect.android.accessibility

import android.accessibilityservice.AccessibilityService
import android.os.Bundle
import android.view.accessibility.AccessibilityEvent
import android.view.accessibility.AccessibilityNodeInfo
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
