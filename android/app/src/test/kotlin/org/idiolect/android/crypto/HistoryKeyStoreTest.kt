package org.idiolect.android.crypto

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * The generate-once / persist-wrapped / reload logic of [HistoryKeyStore], host-tested
 * with a reversible fake [KeyEnvelope] (the real AndroidKeyStore-backed envelope is a
 * device seam). Proves the key is stable across runs and never written in the clear.
 */
class HistoryKeyStoreTest {
    /** Reversible and order-preserving, with `wrap(x) != x` so we can assert on disk. */
    private class FakeEnvelope : KeyEnvelope {
        override fun wrap(plaintext: ByteArray) = ByteArray(plaintext.size) { (plaintext[it] + 1).toByte() }
        override fun unwrap(wrapped: ByteArray) = ByteArray(wrapped.size) { (wrapped[it] - 1).toByte() }
    }

    private fun tempKeyFile(): File =
        File.createTempFile("history-key", ".enc").apply { delete() }

    @Test
    fun generates_a_32_byte_key_and_persists_it_on_first_use() {
        val file = tempKeyFile()
        val key = HistoryKeyStore(FakeEnvelope(), file).loadOrCreate()
        assertEquals(32, key.size)
        assertTrue(file.exists())
    }

    @Test
    fun reload_returns_the_same_key() {
        val file = tempKeyFile()
        val first = HistoryKeyStore(FakeEnvelope(), file).loadOrCreate()
        val second = HistoryKeyStore(FakeEnvelope(), file).loadOrCreate()
        assertArrayEquals(first, second)
    }

    @Test
    fun the_key_is_stored_wrapped_never_in_plaintext() {
        val file = tempKeyFile()
        val fixed = ByteArray(32) { 9 }
        val key = HistoryKeyStore(FakeEnvelope(), file, randomBytes = { _ -> fixed.copyOf() }).loadOrCreate()
        assertArrayEquals(fixed, key)
        assertFalse("raw key must not appear on disk", file.readBytes().contentEquals(fixed))
    }
}
