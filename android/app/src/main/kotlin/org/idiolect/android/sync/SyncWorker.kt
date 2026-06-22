package org.idiolect.android.sync

import android.content.Context
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.idiolect.android.crypto.HistoryKey
import org.idiolect.ffi.IdiolectCore
import org.idiolect.ffi.IdiolectInputMethod
import java.io.File

/**
 * The scheduled outbox push (M6): drain the local sync outbox to the PC. WorkManager
 * runs this off the main thread when its [SyncScheduler] constraints (a network) are met.
 *
 * This is the **native boundary** — it opens its own short-lived [IdiolectCore] over the
 * same `filesDir` (no model loaded; `exportSyncBatch`/`confirmSynced` only touch the store
 * and audio), so it can't be host-tested. Everything it composes is, though: the drain
 * logic ([OutboxPump.drain]), the endpoint ([SecureSyncConfig]), the id ([DeviceId]), the
 * transport ([HttpSyncTransport]), and the scheduling ([SyncSchedulerTest]). The native
 * round-trip is proven by the Rust seam test and the emulator e2e.
 *
 * Outcomes: nothing configured or a clean drain → success; a transport/IO failure →
 * retry (the outbox is intact, delete-after-ACK guarantees no data loss).
 */
class SyncWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val filesDir = applicationContext.filesDir
        // No endpoint paired yet → nothing to ship; succeed so the job isn't retried.
        val settings = SecureSyncConfig.keystoreBacked(filesDir).load() ?: return Result.success()
        val deviceId = DeviceId(File(filesDir, DeviceId.FILE_NAME)).get()

        val historyKey = HistoryKey.load(File(filesDir, HistoryKey.FILE_NAME))
        val core = IdiolectCore(filesDir.absolutePath, historyKey, NoOpInputMethod)
        return try {
            val pump = OutboxPump(
                source = CoreSyncSource(core),
                transport = HttpSyncTransport(settings.baseUrl, settings.token, settings.pin),
                deviceId = deviceId,
            )
            pump.drain()
            Result.success()
        } catch (error: Exception) {
            // Network down / PC unreachable / transient store contention — keep the outbox
            // and let WorkManager back off and retry.
            Result.retry()
        } finally {
            core.close()
        }
    }

    /** The worker drives no UI and receives no pushes; the core needs a sink regardless. */
    private object NoOpInputMethod : IdiolectInputMethod {
        override fun recordingStatus(recording: Boolean) {}
        override fun showPreedit(text: String) {}
        override fun updatePreedit(text: String) {}
        override fun commitText(text: String) {}
        override fun cancelPreedit() {}
        override fun insertText(text: String) {}
        override fun editHistory(id: Long, text: String) {}
        override fun dictationError(message: String) {}
    }
}
