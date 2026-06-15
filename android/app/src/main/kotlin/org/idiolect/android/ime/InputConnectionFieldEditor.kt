package org.idiolect.android.ime

import android.os.Handler
import android.os.Looper
import android.view.inputmethod.InputConnection

/**
 * [FieldEditor] backed by a live `InputConnection`.
 *
 * Every edit is posted to the main thread — the only thread an `InputConnection` may
 * be touched from. Live preedits arrive on the dictation pump thread while commits
 * arrive on the main thread; posting both through one main-thread handler keeps them
 * correctly ordered.
 *
 * This is a thin framework seam (pure delegation to `InputConnection`) with no
 * headless test surface — the mapping logic it serves is unit-tested via [FieldEditor]
 * fakes ([IdiolectImeCallbackTest]); this delegation is exercised by the emulator e2e.
 */
class InputConnectionFieldEditor(
    private val connection: InputConnection,
    private val main: Handler = Handler(Looper.getMainLooper()),
) : FieldEditor {
    override fun setComposingText(text: String) {
        post { connection.setComposingText(text, 1) }
    }

    override fun commitText(text: String) {
        post { connection.commitText(text, 1) }
    }

    override fun finishComposingText() {
        post { connection.finishComposingText() }
    }

    override fun deleteBackward() {
        post { connection.deleteSurroundingText(1, 0) }
    }

    // Tap-driven (already on the main thread) and synchronous: applied directly, not
    // posted — `setSelection` must take effect before the mode swap, and `fieldText`
    // must return a value.
    override fun setSelection(start: Int, end: Int) {
        connection.setSelection(start, end)
    }

    override fun fieldText(): String {
        val before = connection.getTextBeforeCursor(MAX_FIELD_CHARS, 0) ?: ""
        val selected = connection.getSelectedText(0) ?: ""
        val after = connection.getTextAfterCursor(MAX_FIELD_CHARS, 0) ?: ""
        return "$before$selected$after"
    }

    private fun post(action: () -> Unit) {
        main.post(action)
    }

    private companion object {
        // Generous cap for reading the whole field back; real dictation fields are tiny.
        const val MAX_FIELD_CHARS = 100_000
    }
}
