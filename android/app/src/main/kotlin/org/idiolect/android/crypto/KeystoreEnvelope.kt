package org.idiolect.android.crypto

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * [KeyEnvelope] backed by a hardware-backed AES-256-GCM key in the AndroidKeyStore.
 * The key-encryption key is generated once under [alias] and never leaves the Keystore;
 * only the (un)wrapped payload crosses into app memory. AOSP/FOSS only — no Google Play
 * Services, so it works on GrapheneOS.
 *
 * Wrapped layout: `[12-byte GCM IV][ciphertext+tag]`. This is a thin framework seam
 * (the AndroidKeyStore provider only exists on a device); the persistence/round-trip
 * logic it serves is host-tested via [HistoryKeyStore] with a fake envelope, and this
 * class is exercised by the on-device bridge test.
 */
class KeystoreEnvelope(private val alias: String = DEFAULT_ALIAS) : KeyEnvelope {
    override fun wrap(plaintext: ByteArray): ByteArray {
        val cipher = Cipher.getInstance(TRANSFORMATION).apply { init(Cipher.ENCRYPT_MODE, kek()) }
        val iv = cipher.iv
        return iv + cipher.doFinal(plaintext)
    }

    override fun unwrap(wrapped: ByteArray): ByteArray {
        val iv = wrapped.copyOfRange(0, IV_BYTES)
        val body = wrapped.copyOfRange(IV_BYTES, wrapped.size)
        val cipher = Cipher.getInstance(TRANSFORMATION)
            .apply { init(Cipher.DECRYPT_MODE, kek(), GCMParameterSpec(TAG_BITS, iv)) }
        return cipher.doFinal(body)
    }

    /** The hardware-backed AES KEK, created on first use. */
    private fun kek(): SecretKey {
        val store = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (store.getEntry(alias, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                alias,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(KEK_BITS)
                .build(),
        )
        return generator.generateKey()
    }

    private companion object {
        const val KEYSTORE = "AndroidKeyStore"
        const val DEFAULT_ALIAS = "idiolect.history.kek"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val IV_BYTES = 12
        const val TAG_BITS = 128
        const val KEK_BITS = 256
    }
}
