package org.idiolect.android.ime

import org.idiolect.ffi.IdiolectInputMethod

/**
 * Maps the Rust core's [IdiolectInputMethod] push callbacks onto the IME's field and
 * UI operations. Pure logic: field edits go through [FieldEditor] (backed by the live
 * `InputConnection`), UI pushes through [ImeUiHost].
 *
 * Field edits are no-ops when no field is focused ([editorProvider] returns `null`) —
 * e.g. a push that arrives between fields — so they never crash.
 *
 * **Threading / re-entrancy:** the core invokes these synchronously while holding its
 * lock (see the FFI contract). This mapping never calls back into the core, so it
 * cannot deadlock. The service's [FieldEditor]/[ImeUiHost] implementations are
 * responsible for marshalling `InputConnection`/UI work onto the main thread.
 */
class IdiolectImeCallback(
    private val editorProvider: () -> FieldEditor?,
    private val ui: ImeUiHost,
) : IdiolectInputMethod {
    override fun recordingStatus(recording: Boolean) = ui.onRecordingChanged(recording)

    override fun showPreedit(text: String) {
        editorProvider()?.setComposingText(text)
    }

    override fun updatePreedit(text: String) {
        editorProvider()?.setComposingText(text)
    }

    override fun commitText(text: String) {
        editorProvider()?.commitText(text)
        // A take landed: tell the UI so it can build the correction strip. (A history
        // reinsert uses insertText, which is not a fresh take and does not notify.)
        ui.onCommit(text)
    }

    override fun cancelPreedit() {
        editorProvider()?.finishComposingText()
    }

    override fun insertText(text: String) {
        editorProvider()?.commitText(text)
    }

    override fun editHistory(id: Long, text: String) = ui.onEditHistory(id, text)

    override fun dictationError(message: String) = ui.onDictationError(message)
}
