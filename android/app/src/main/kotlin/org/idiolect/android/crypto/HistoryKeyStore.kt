package org.idiolect.android.crypto

import java.io.File
import java.security.SecureRandom

/**
 * Provides the stable 32-byte key for at-rest history encryption (the ChaCha key the
 * Rust core uses). Generated once with a CSPRNG and persisted **wrapped** by [envelope]
 * in app-private `filesDir`, so the raw key never sits on disk in the clear; on later
 * runs it is unwrapped back. This is the Android counterpart of the desktop `FileKey`
 * (unix perms are meaningless in the sandbox, so we lean on the Keystore instead).
 */
class HistoryKeyStore(
    private val envelope: KeyEnvelope,
    private val keyFile: File,
    private val randomBytes: (Int) -> ByteArray = { n -> ByteArray(n).also { SecureRandom().nextBytes(it) } },
) {
    fun loadOrCreate(): ByteArray {
        if (keyFile.exists()) {
            return envelope.unwrap(keyFile.readBytes())
        }
        val key = randomBytes(KEY_BYTES)
        keyFile.parentFile?.mkdirs()
        keyFile.writeBytes(envelope.wrap(key))
        return key
    }

    private companion object {
        const val KEY_BYTES = 32
    }
}
