package org.idiolect.android.core

import org.idiolect.ffi.IdiolectInputMethod

/**
 * An [IdiolectInputMethod] that ignores every core push. A base for sinks that care about only a
 * couple of callbacks — e.g. a headless recognition take wants `commitText` and `dictationError`
 * but none of the preedit/history pushes the IME view handles. Override just what you need.
 */
open class NoopInputMethod : IdiolectInputMethod {
    override fun recordingStatus(recording: Boolean) {}
    override fun showPreedit(text: String) {}
    override fun updatePreedit(text: String) {}
    override fun commitText(text: String) {}
    override fun cancelPreedit() {}
    override fun insertText(text: String) {}
    override fun editHistory(id: Long, text: String) {}
    override fun dictationError(message: String) {}
}
