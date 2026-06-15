package org.idiolect.android.ime

/**
 * The UI-facing side of the core's push callbacks — everything that is not direct
 * field editing. Implemented by the IME service to drive its mic indicator, open the
 * review dialog, and surface decode failures.
 */
interface ImeUiHost {
    /** Authoritative recording state changed (the single source of truth). */
    fun onRecordingChanged(recording: Boolean)

    /** A take committed its text — seed the correction strip from it. */
    fun onCommit(text: String)

    /** Open the review dialog seeded with a stored history entry. */
    fun onEditHistory(id: Long, text: String)

    /** A take failed to decode (no model / engine error): tell the user, once. */
    fun onDictationError(message: String)
}
