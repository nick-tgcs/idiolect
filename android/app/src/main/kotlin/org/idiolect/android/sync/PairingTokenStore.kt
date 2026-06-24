package org.idiolect.android.sync

import org.idiolect.android.crypto.KeyEnvelope
import org.idiolect.android.crypto.KeystoreEnvelope
import java.io.File

/**
 * Stores the per-device sync bearer token at rest, wrapped by [envelope]. On device the
 * envelope is a hardware-backed AndroidKeyStore key ([KeystoreEnvelope]) under its own KEK
 * alias, so the token never sits on disk in the clear; tests substitute a reversible fake.
 *
 * This is the sync-token analogue of `crypto/HistoryKeyStore` (which protects the at-rest
 * history key the same way). The wrap/unwrap round-trip is host-tested with a fake envelope;
 * the AndroidKeyStore provider itself is a device-only seam, exercised by the bridge test.
 */
class PairingTokenStore(
    private val envelope: KeyEnvelope,
    private val tokenFile: File,
) {
    /** Persist [token] wrapped, replacing any previous one. */
    fun save(token: String) {
        tokenFile.parentFile?.mkdirs()
        tokenFile.writeBytes(envelope.wrap(token.toByteArray(Charsets.UTF_8)))
    }

    /** The unwrapped token, or `null` if none has been paired yet. */
    fun load(): String? =
        tokenFile.takeIf { it.exists() }
            ?.let { envelope.unwrap(it.readBytes()).toString(Charsets.UTF_8) }

    /** Wipe the at-rest token (unpairing). A no-op when nothing was saved. */
    fun clear() {
        tokenFile.delete()
    }

    companion object {
        const val FILE_NAME = "sync.token.enc"

        /** Distinct from the history KEK so the two secrets are independently protected. */
        const val KEK_ALIAS = "idiolect.sync.token.kek"

        /** The on-device store: token wrapped by a dedicated AndroidKeyStore key. */
        fun keystoreBacked(tokenFile: File) =
            PairingTokenStore(KeystoreEnvelope(KEK_ALIAS), tokenFile)
    }
}
