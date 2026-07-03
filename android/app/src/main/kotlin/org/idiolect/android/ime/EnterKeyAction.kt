package org.idiolect.android.ime

import android.view.inputmethod.EditorInfo

/**
 * Decides what the on-screen Enter key does for a given field, mirroring how the
 * system Enter key behaves: if the field declares an editor action (Send, Search,
 * Go, Done, Next, …) and does not suppress it, perform that action; otherwise insert
 * a plain newline (so multi-line notes keep working).
 *
 * Pure, host-JVM-testable: it reads only the `EditorInfo.imeOptions` bitfield, so the
 * keyboard wiring stays a thin call-through.
 */
object EnterKeyAction {
    sealed interface Decision {
        /** Perform the field's declared editor action (`performEditorAction`). */
        data class PerformAction(val actionId: Int) : Decision

        /** Insert a newline (`commitText("\n")`). */
        data object InsertNewline : Decision
    }

    fun decide(imeOptions: Int): Decision {
        val action = imeOptions and EditorInfo.IME_MASK_ACTION
        val suppressed = imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION != 0
        val hasAction =
            action != EditorInfo.IME_ACTION_NONE && action != EditorInfo.IME_ACTION_UNSPECIFIED
        return if (hasAction && !suppressed) {
            Decision.PerformAction(action)
        } else {
            Decision.InsertNewline
        }
    }
}
