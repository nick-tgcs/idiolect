package org.idiolect.android.ime

/** A key on the edit-mode keyboard. Pure data — the rendering is the GUI seam. */
sealed interface Key {
    /** A letter/symbol key carrying its lower- and upper-case forms. */
    data class Character(val lower: String, val upper: String) : Key

    /** One-shot upper-case for the next character. */
    data object Shift : Key

    /** Delete the character before the cursor. */
    data object Backspace : Key

    data object Space : Key

    data object Enter : Key

    /** The `🎤` key: return to voice mode in place. */
    data object SwitchToVoice : Key
}

/** The v1 tap-only QWERTY layout, as rows of [Key]s. */
object KeyboardLayout {
    private fun row(letters: String): List<Key> =
        letters.map { Key.Character(it.toString(), it.toString().uppercase()) }

    val QWERTY: List<List<Key>> = listOf(
        row("qwertyuiop"),
        row("asdfghjkl"),
        listOf(Key.Shift) + row("zxcvbnm") + Key.Backspace,
        listOf(Key.SwitchToVoice, Key.Space, Key.Character(".", "."), Key.Enter),
    )
}
