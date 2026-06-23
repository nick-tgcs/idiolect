package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The correction strip splits a committed take into tappable word chips, each carrying
 * the char range it occupies in the take so a tap can select exactly that word in the
 * field (plan §1.4). Pure tokenisation, host-tested.
 */
class CorrectionStripTest {
    @Test
    fun splits_a_take_into_words_with_their_ranges() {
        val take = "send him the note"
        val chips = CorrectionStrip.words(take)
        assertEquals(listOf("send", "him", "the", "note"), chips.map { it.text })
        assertEquals(WordChip("send", 0, 4), chips[0])
        assertEquals(WordChip("note", 13, 17), chips[3])
        // The range is exact: substring(start, end) round-trips each chip.
        chips.forEach { assertEquals(it.text, take.substring(it.start, it.end)) }
    }

    @Test
    fun collapses_runs_of_whitespace_and_keeps_offsets() {
        val take = "  hi   there "
        val chips = CorrectionStrip.words(take)
        assertEquals(listOf("hi", "there"), chips.map { it.text })
        assertEquals(WordChip("hi", 2, 4), chips[0])
        assertEquals(WordChip("there", 7, 12), chips[1])
    }

    @Test
    fun punctuation_stays_attached_to_its_word() {
        val take = "their, friend"
        val chips = CorrectionStrip.words(take)
        assertEquals(listOf("their,", "friend"), chips.map { it.text })
        chips.forEach { assertEquals(it.text, take.substring(it.start, it.end)) }
    }

    @Test
    fun blank_input_yields_no_chips() {
        assertTrue(CorrectionStrip.words("").isEmpty())
        assertTrue(CorrectionStrip.words("   ").isEmpty())
    }
}
