package org.idiolect.android.ime

/**
 * Turns "fix it simply" and "ship a learning to the PC" into one path (plan §1.4, the
 * crux). It remembers the committed take, selects a tapped word's range and flips to
 * edit mode so the next keystroke replaces it, and on [capture] reads the **whole**
 * field back and records a raw→corrected pair via the core's `report_correction` — but
 * only when the text actually changed.
 *
 * Reading the corrected text back from the field (never our own optimistic state) mirrors
 * the desktop capture. Synchronized because [onTakeCommitted] arrives on the core's
 * callback thread while [selectWord]/[capture] are driven by main-thread taps.
 */
class CorrectionCapture(
    private val editor: () -> FieldEditor?,
    private val reportCorrection: (String) -> Unit,
    private val onEnterEdit: () -> Unit,
) {
    private var baseline: String? = null
    private var chips: List<WordChip> = emptyList()

    @Synchronized
    fun currentChips(): List<WordChip> = chips

    /**
     * Drop any pending baseline and chips. Used when the focus moves through a password/PIN
     * field: the committed take must not be carried forward and later amended with an unrelated
     * field's text (which would mint a syncable pair from a secret). A following [capture]
     * reports nothing until a new take arms the baseline again.
     */
    @Synchronized
    fun disarm() {
        baseline = null
        chips = emptyList()
    }

    /** A take committed: this becomes the raw baseline and the strip's chips. */
    @Synchronized
    fun onTakeCommitted(text: String): List<WordChip> {
        baseline = text
        chips = CorrectionStrip.words(text)
        return chips
    }

    /** Select the tapped word's range in the field and flip to edit mode. */
    @Synchronized
    fun selectWord(index: Int) {
        val chip = chips.getOrNull(index) ?: return
        editor()?.setSelection(chip.start, chip.end)
        onEnterEdit()
    }

    /**
     * Read the field back; if it differs from the committed take, record the pair and
     * advance the baseline (so re-capturing the same text is a no-op). Returns whether a
     * correction was recorded.
     */
    @Synchronized
    fun capture(): Boolean {
        val current = editor()?.fieldText() ?: return false
        val base = baseline ?: return false
        if (current == base) return false
        reportCorrection(current)
        baseline = current
        chips = CorrectionStrip.words(current)
        return true
    }
}
