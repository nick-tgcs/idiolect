package org.idiolect.android.sync

import android.content.Context
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import java.util.concurrent.TimeUnit

/**
 * Schedules the deferred outbox push ([SyncWorker]). Triggered after a correction is
 * captured (the IME) and after a successful setup; WorkManager defers it until the device
 * has a network and runs it off the main thread. Uses [ExistingWorkPolicy.KEEP] under a
 * single unique name so a burst of corrections collapses into one pending job rather than
 * piling up.
 *
 * WorkManager is AndroidX-only (no Google Play Services / FCM), the GrapheneOS hard
 * requirement.
 */
object SyncScheduler {
    const val UNIQUE_WORK_NAME = "idiolect-outbox-sync"

    /** Sync only on a network — battery/data friendly, and pointless offline. */
    private fun constraints(): Constraints =
        Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build()

    fun enqueue(context: Context) {
        val request = OneTimeWorkRequestBuilder<SyncWorker>()
            .setConstraints(constraints())
            .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
            .build()
        WorkManager.getInstance(context)
            .enqueueUniqueWork(UNIQUE_WORK_NAME, ExistingWorkPolicy.KEEP, request)
    }
}
