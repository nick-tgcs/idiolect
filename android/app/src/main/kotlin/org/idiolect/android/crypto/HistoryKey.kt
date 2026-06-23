package org.idiolect.android.crypto

import java.io.File

/**
 * The on-device entry point for the at-rest history key: a stable 32-byte key, wrapped
 * by the AndroidKeyStore ([KeystoreEnvelope]) in [keyFile]. Pass the result to
 * `IdiolectCore`'s constructor to enable history encryption.
 */
object HistoryKey {
    /** Default key-file name under the app's private `filesDir`. */
    const val FILE_NAME = "history.key.enc"

    fun load(keyFile: File): ByteArray =
        HistoryKeyStore(KeystoreEnvelope(), keyFile).loadOrCreate()
}
