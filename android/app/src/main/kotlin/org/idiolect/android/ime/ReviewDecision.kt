package org.idiolect.android.ime

/**
 * The pure decisions behind the 👁 review flow. A finished take, when review is on, opens
 * the centred [ReviewActivity] (edited with the user's own keyboard) instead of landing in
 * the field; on Insert the edit is recorded as a raw→corrected training pair (the whole
 * point) and the corrected text is typed into the field when idiolect regains focus.
 */
object ReviewDecision {
    /** Review a finished take before it lands — when 👁 is on and it isn't a per-phrase
     *  continuous commit (reviewing every pause would be absurd). */
    fun shouldReview(reviewEnabled: Boolean, continuous: Boolean): Boolean =
        reviewEnabled && !continuous

    /** Whether the reviewed text is a genuine correction worth recording: it changed (beyond
     *  surrounding whitespace) and isn't blank. */
    fun isCorrection(raw: String, edited: String): Boolean =
        edited.isNotBlank() && edited.trim() != raw.trim()

    /** The text to type into the field on Insert, or null when there's nothing to insert. */
    fun textToInsert(edited: String): String? = edited.takeIf { it.isNotBlank() }
}

/**
 * The deferred-insert channel: the [ReviewActivity] stashes the approved text on Insert, and
 * the IME types it into the original field when it next regains focus (the user flips back
 * to idiolect after editing in their own keyboard). Same process, so a guarded singleton is
 * enough; the capture of the training pair has already happened by then, independently.
 */
object PendingInsert {
    @Volatile
    private var text: String? = null

    @Synchronized
    fun set(value: String) {
        text = value
    }

    /** Take and clear the pending text (null if nothing is pending). */
    @Synchronized
    fun take(): String? {
        val value = text
        text = null
        return value
    }
}
