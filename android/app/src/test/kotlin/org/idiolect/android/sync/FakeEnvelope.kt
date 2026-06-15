package org.idiolect.android.sync

import org.idiolect.android.crypto.KeyEnvelope

/**
 * Reversible, order-preserving stand-in for the AndroidKeyStore-backed [KeyEnvelope]
 * (the real one is a device seam). `wrap(x) != x` so a test can assert the secret never
 * lands on disk in the clear, while `unwrap(wrap(x)) == x` keeps the round-trip honest.
 * Mirrors the fake in `crypto/HistoryKeyStoreTest`.
 */
internal class FakeEnvelope : KeyEnvelope {
    override fun wrap(plaintext: ByteArray) = ByteArray(plaintext.size) { (plaintext[it] + 1).toByte() }

    override fun unwrap(wrapped: ByteArray) = ByteArray(wrapped.size) { (wrapped[it] - 1).toByte() }
}
