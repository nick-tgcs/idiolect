package org.idiolect.android.crypto

/**
 * Wraps/unwraps secret bytes with a protected key. On device this is backed by a
 * hardware-backed AndroidKeyStore key ([KeystoreEnvelope]) so the wrapping key never
 * leaves secure storage; tests substitute a fake. The contract: `unwrap(wrap(x)) == x`,
 * and `wrap(x)` is not `x` (so the secret is never persisted in the clear).
 */
interface KeyEnvelope {
    fun wrap(plaintext: ByteArray): ByteArray

    fun unwrap(wrapped: ByteArray): ByteArray
}
