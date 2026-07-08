package org.idiolect.android.ime

/**
 * The two flanking edit keys of the mic surface (Option A): ⌫ delete and ⏎ enter. A pure
 * dispatcher onto [FieldEditor] so the key→field wiring is host-JVM testable — the View just
 * forwards taps here. Both are no-ops when no field is focused (`fieldEditor()` is `null`
 * between fields).
 */
object EditKeys {
    /** ⌫ — delete the character before the cursor. */
    fun delete(editor: FieldEditor?) {
        editor?.deleteBackward()
    }

    /** ⏎ — perform the field's editor action, or insert a newline (see [EnterAction]). */
    fun enter(editor: FieldEditor?, imeOptions: Int) {
        editor ?: return
        when (val action = EnterAction.forEditor(imeOptions)) {
            is EnterAction.Editor -> editor.performEditorAction(action.actionId)
            EnterAction.Newline -> editor.commitText("\n")
        }
    }
}
