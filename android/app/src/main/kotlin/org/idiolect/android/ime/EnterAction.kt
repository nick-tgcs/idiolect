package org.idiolect.android.ime

import android.view.inputmethod.EditorInfo

/**
 * What the ⏎ enter key does in the focused field, decided purely from its `EditorInfo`. A field
 * that declares a concrete IME action (go, search, send, next, done, previous) — or supplies a
 * *custom* action via `EditorInfo.actionLabel`/`actionId` — wants that action performed; anything
 * else — an unspecified or explicitly-none action, or a field that opts out with
 * `IME_FLAG_NO_ENTER_ACTION` — gets a literal newline. Pure so it's host-JVM unit-testable; the
 * framework `EditorInfo` constants are compile-time `int`s (inlined), so no Android runtime is
 * needed.
 */
sealed interface EnterAction {
    /** Perform the field's declared editor action (`InputConnection.performEditorAction`). */
    data class Editor(val actionId: Int) : EnterAction

    /** Insert a literal newline — the field has no actionable enter. */
    object Newline : EnterAction

    companion object {
        /**
         * @param imeOptions the focused field's `EditorInfo.imeOptions`.
         * @param customActionId the field's `EditorInfo.actionId` when it declares a custom action
         *   (i.e. `actionLabel != null`), else `null`. A custom action isn't encoded in the masked
         *   `imeOptions` bits, so it must be threaded in separately or it'd be lost as a newline.
         */
        fun forEditor(imeOptions: Int, customActionId: Int? = null): EnterAction {
            // Opt-out wins over any action — the user explicitly asked enter to mean newline.
            if (imeOptions and EditorInfo.IME_FLAG_NO_ENTER_ACTION != 0) return Newline
            if (customActionId != null) return Editor(customActionId)
            val action = imeOptions and EditorInfo.IME_MASK_ACTION
            val actionable =
                action != EditorInfo.IME_ACTION_NONE &&
                    action != EditorInfo.IME_ACTION_UNSPECIFIED
            return if (actionable) Editor(action) else Newline
        }
    }
}
