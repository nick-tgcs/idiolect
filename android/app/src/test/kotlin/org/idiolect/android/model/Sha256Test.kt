package org.idiolect.android.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class Sha256Test {
    private fun tempFile(bytes: ByteArray): File =
        File.createTempFile("sha", ".bin").apply { writeBytes(bytes) }

    @Test
    fun matches_the_known_vector() {
        // SHA-256("abc") — and the same value the Rust file_sha256_hex produces.
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            Sha256.ofFile(tempFile("abc".toByteArray())),
        )
    }

    @Test
    fun high_bit_bytes_stay_two_hex_chars_each() {
        // Regression: a naive `%02x` on a signed byte sign-extends to 8 chars.
        val digest = Sha256.ofFile(tempFile(ByteArray(50) { 0xAB.toByte() }))
        assertEquals(64, digest.length)
        assertTrue(digest.all { it.isDigit() || it in 'a'..'f' })
    }
}
