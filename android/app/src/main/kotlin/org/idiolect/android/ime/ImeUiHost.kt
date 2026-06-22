package org.idiolect.android.ime

/**
 * The UI-facing side of the core's push callbacks — everything that is not direct
 * field editing. Implemented by the IME service to drive its mic indicator, seed the
 * correction strip, and surface decode failures.
 */
interface ImeUiHost {
    /** Authoritative recording state changed (the single source of truth). */
    fun onRecordingChanged(recording: Boolean)

    /** A take committed its text — seed the correction strip from it. */
    fun onCommit(text: String)

    /**
     * Whether review mode (👁) is on: a finished take should be reviewed/edited before it
     * lands in the field, rather than typed directly. Read once per take commit.
     */
    fun isReviewEnabled(): Boolean

    /**
     * Live partial transcript during a review-mode take. In review mode the words must NOT be
     * typed into the host field (they'd land in the target app, then get clawed back) — they
     * stream onto idiolect's own review surface instead. An empty [text] clears the surface
     * (a cancelled preedit). Only called when [isReviewEnabled] is true for the take.
     */
    fun onLivePreedit(text: String)

    /**
     * Review mode intercepted a finished take: open the centred review surface for [text].
     * The take is already persisted (audio + raw transcript + a history id); the user edits
     * it with their own keyboard, the edit is recorded as a training pair, and the approved
     * text is typed into the field when it refocuses.
     */
    fun onReviewRequested(text: String)

    /** Open the review dialog seeded with a stored history entry. */
    fun onEditHistory(id: Long, text: String)

    /** A take failed to decode (no model / engine error): tell the user, once. */
    fun onDictationError(message: String)
}
