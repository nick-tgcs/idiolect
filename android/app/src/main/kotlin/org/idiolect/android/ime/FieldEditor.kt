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
}
