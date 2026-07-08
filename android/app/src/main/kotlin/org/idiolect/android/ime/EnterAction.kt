package org.idiolect.android.ime

import android.view.inputmethod.EditorInfo

/**
 * What the ⏎ enter key does in the focused field, decided purely from its
 * `EditorInfo.imeOptions`. A field that declares a concrete IME action (go, search, send,
 * next, done, previous) wants that action performed; anything else — an unspecified or
 * explicitly-none action, or a field that opts out with `IME_FLAG_NO_ENTER_ACTION` — gets a
 * literal newline. Pure so it's host-JVM unit-testable; the framework `EditorInfo` constants
 * are compile-time `int`s (inlined), so no Android runtime is needed.
 */
sealed interface EnterAction {
    /** Perform the field's declared editor action (`InputConnection.performEditorAction`). */
    data class Editor(val actionId: Int) : EnterAction

    /** Insert a literal newline — the field has no actionable enter. */
    object Newline : EnterAction

    companion object {
        fun forEditor(imeOptions: Int): EnterAction {
            val action = imeOptions and EditorInfo.IME_MASK_ACTION
            val optedOut = imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION != 0
            val actionable =
                !optedOut &&
                    action != EditorInfo.IME_ACTION_NONE &&
                    action != EditorInfo.IME_ACTION_UNSPECIFIED
            return if (actionable) Editor(action) else Newline
        }
    }
}
