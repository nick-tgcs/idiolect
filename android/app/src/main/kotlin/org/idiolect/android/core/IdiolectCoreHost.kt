package org.idiolect.android.core

import android.content.Context
import org.idiolect.android.crypto.HistoryKey
import org.idiolect.ffi.IdiolectCore
import java.io.File

/**
 * Process-wide owner of the single [IdiolectCore] and its [CoreCallbackRouter]. Pulling the
 * core out of [org.idiolect.android.ime.IdiolectImeService] lets it outlive that service —
 * which Android destroys whenever the user switches to another keyboard — so a take's
 * correction can still be captured while the user edits in their own keyboard during review
 * (capture is the whole point; see the review flow).
 *
 * Construction is the only framework-touching part (it loads the hardware-wrapped history
 * key and spins up the native core), so it stays a thin seam; the routing *logic* lives in
 * the unit-tested [CoreCallbackRouter].
 */
class IdiolectCoreHost private constructor(context: Context) {
    val router = CoreCallbackRouter()
    val core: IdiolectCore = run {
        // At-rest encryption of the history projection: a 32-byte key wrapped by the
        // hardware-backed AndroidKeyStore, generated once under filesDir.
        val historyKey = HistoryKey.load(File(context.filesDir, HistoryKey.FILE_NAME))
        IdiolectCore(context.filesDir.absolutePath, historyKey, router)
    }

    companion object {
        @Volatile
        private var instance: IdiolectCoreHost? = null
        private var refs = 0

        /**
         * Take a reference to the process-wide host (creating it on first use). Each holder —
         * the IME service while it's alive, the review Activity while it's shown — must
         * [release] exactly once. The core stays alive while any reference is held, so it
         * survives the IME being torn down on a keyboard switch (the review captures a
         * correction while the user edits in their own keyboard).
         */
        @Synchronized
        fun acquire(context: Context): IdiolectCoreHost {
            refs++
            return instance ?: IdiolectCoreHost(context.applicationContext).also { instance = it }
        }

        /** Drop a reference; when the last one goes, close the native core. Idempotent-safe. */
        @Synchronized
        fun release() {
            if (refs > 0) refs--
            if (refs == 0) {
                instance?.core?.close()
                instance = null
            }
        }
    }
}
