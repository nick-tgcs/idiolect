package org.idiolect.android.settings

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The display formatting for the pinned cert's SPKI fingerprint shown on the paired-PC card.
 * A 64-hex-char pin is unreadable as one run; grouping it into quads lets a human compare it
 * against the PC's `--pair` output digit-by-digit (the whole point of pin verification). Pure
 * string logic, so it is host-tested exhaustively and never throws on a malformed pin.
 */
class CertFingerprintTest {
    private val realPin =
        "94e32367cdca173650e3dbd73f4a2dc657e0951c9cc0974556a6909fe5216c3c"

    @Test
    fun groups_a_64_char_pin_into_sixteen_quads() {
        assertEquals(
            "94e3 2367 cdca 1736 50e3 dbd7 3f4a 2dc6 57e0 951c 9cc0 9745 56a6 909f e521 6c3c",
            CertFingerprint.grouped(realPin),
        )
    }

    @Test
    fun a_trailing_partial_group_is_kept_whole() {
        // Never drop characters: a length not divisible by four leaves a short final group.
        assertEquals("dead beef a", CertFingerprint.grouped("deadbeefa"))
    }

    @Test
    fun an_empty_fingerprint_groups_to_empty() {
        assertEquals("", CertFingerprint.grouped(""))
    }

    @Test
    fun short_shows_only_the_last_quad_for_compact_display() {
        assertEquals("…6c3c", CertFingerprint.short(realPin))
    }

    @Test
    fun short_of_a_tiny_value_is_left_as_is() {
        assertEquals("ab", CertFingerprint.short("ab"))
    }
}
