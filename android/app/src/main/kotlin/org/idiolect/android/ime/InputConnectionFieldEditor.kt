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

    private fun post(action: () -> Unit) {
        main.post(action)
    }
}
