package org.idiolect.android.ime

/** A tappable word in the correction strip and the char range it occupies in the take. */
data class WordChip(val text: String, val start: Int, val end: Int)

/**
 * Splits a committed take into word chips. A chip is a maximal run of non-whitespace
 * (so punctuation stays attached to its word); its [WordChip.start]/[WordChip.end] are
 * exact offsets into the take, so a tap can select precisely that word in the field.
 */
object CorrectionStrip {
    fun words(take: String): List<WordChip> {
        val chips = mutableListOf<WordChip>()
        var start = -1
        take.forEachIndexed { i, c ->
            if (c.isWhitespace()) {
                if (start >= 0) {
                    chips.add(WordChip(take.substring(start, i), start, i))
                    start = -1
                }
            } else if (start < 0) {
                start = i
            }
        }
        if (start >= 0) chips.add(WordChip(take.substring(start), start, take.length))
        return chips
    }
}
