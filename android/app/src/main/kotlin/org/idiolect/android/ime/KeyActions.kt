package org.idiolect.android.ime

/**
 * The always-available editing keys the voice keyboard exposes alongside the mic —
 * the common things you reach for right after a take lands: a backspace to fix the
 * tail, and an Enter that does what the field expects (send the message, or a
 * newline). Pure wiring over the [FieldEditor] seam so it is host-JVM-testable; the
 * IME service supplies the live editor and the field's `imeOptions`.
 */
class KeyActions(private val editorProvider: () -> FieldEditor?) {
    /** Delete the character before the cursor. */
    fun backspace() {
        editorProvider()?.deleteBackward()
    }

    /**
     * Enter for the given field: perform its declared editor action, or insert a
     * newline when it declares none (or suppresses the action).
     */
    fun enter(imeOptions: Int) {
        val editor = editorProvider() ?: return
        when (val decision = EnterKeyAction.decide(imeOptions)) {
            is EnterKeyAction.Decision.PerformAction -> editor.performEditorAction(decision.actionId)
            EnterKeyAction.Decision.InsertNewline -> editor.commitText("\n")
        }
    }
}
