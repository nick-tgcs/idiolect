package org.idiolect.android.ime

/**
 * The edit-mode key reducer. Maps a [Key] tap onto [FieldEditor] operations against the
 * live field, holding the one-shot shift state (shift upper-cases the next character,
 * then clears — standard soft-keyboard behaviour; caps-lock is future).
 *
 * Field edits are no-ops when no field is focused ([editor] returns `null`) so a stray
 * tap between fields never crashes; the mode switch and shift state are field-independent.
 */
class EditKeyboard(
    private val editor: () -> FieldEditor?,
    private val onSwitchToVoice: () -> Unit,
) {
    var isShifted: Boolean = false
        private set

    fun onKey(key: Key) {
        when (key) {
            is Key.Character -> {
                editor()?.commitText(if (isShifted) key.upper else key.lower)
                isShifted = false
            }
            Key.Shift -> isShifted = !isShifted
            Key.Backspace -> editor()?.deleteBackward()
            Key.Space -> editor()?.commitText(" ")
            Key.Enter -> editor()?.commitText("\n")
            Key.SwitchToVoice -> onSwitchToVoice()
        }
    }
}
