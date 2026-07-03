package org.idiolect.android.ime

/**
 * The narrow set of text-field operations the IME performs, abstracted away from
 * `android.view.inputmethod.InputConnection` so the callback mapping is pure,
 * host-JVM-testable logic. The IME service supplies an implementation backed by the
 * live `InputConnection` (the thin, framework-only seam).
 */
interface FieldEditor {
    /** Show/replace the live preedit region (`InputConnection.setComposingText`). */
    fun setComposingText(text: String)

    /** Finalise text into the field at the cursor (`InputConnection.commitText`). */
    fun commitText(text: String)

    /** Clear the preedit without committing (`InputConnection.finishComposingText`). */
    fun finishComposingText()

    /** Delete the character before the cursor (`InputConnection.deleteSurroundingText`). */
    fun deleteBackward()

    /**
     * Run the field's editor action — Send/Search/Go/Done, etc.
     * (`InputConnection.performEditorAction`). The Enter key uses this when the field
     * declares an action; a plain newline goes through [commitText] instead.
     */
    fun performEditorAction(actionId: Int)

    /** Select a char range in the field (`InputConnection.setSelection`) — for tap-to-fix. */
    fun setSelection(start: Int, end: Int)

    /**
     * The entire current field text (`InputConnection.getTextBefore/Selected/After`) —
     * the ground truth read back when capturing a correction. Main-thread only.
     */
    fun fieldText(): String
}
