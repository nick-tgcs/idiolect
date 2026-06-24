package org.idiolect.android.sync

import android.content.Context
import android.util.Log
import androidx.test.core.app.ApplicationProvider
import androidx.work.Configuration
import androidx.work.NetworkType
import androidx.work.WorkInfo
import androidx.work.WorkManager
import androidx.work.testing.SynchronousExecutor
import androidx.work.testing.WorkManagerTestInitHelper
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * The scheduler policy, verified against WorkManager's test harness on the host JVM (no
 * emulator): the worker is enqueued under a single unique name and is gated on a network.
 * The worker body itself ([SyncWorker]) is native-bound, so it never runs here — the
 * CONNECTED constraint is left unmet, leaving the work ENQUEUED for inspection.
 */
@RunWith(RobolectricTestRunner::class)
class SyncSchedulerTest {
    private fun context(): Context = ApplicationProvider.getApplicationContext()

    @Before
    fun initWorkManager() {
        val config = Configuration.Builder()
            .setExecutor(SynchronousExecutor())
            .setMinimumLoggingLevel(Log.DEBUG)
            .build()
        WorkManagerTestInitHelper.initializeTestWorkManager(context(), config)
    }

    @Test
    fun enqueue_schedules_one_network_gated_unique_sync_job() {
        SyncScheduler.enqueue(context())

        val infos = WorkManager.getInstance(context())
            .getWorkInfosForUniqueWork(SyncScheduler.UNIQUE_WORK_NAME)
            .get()

        assertEquals("a single unique sync job", 1, infos.size)
        val info = infos.single()
        assertEquals(WorkInfo.State.ENQUEUED, info.state)
        assertEquals(
            "sync only runs on a network (battery/data friendly, FOSS)",
            NetworkType.CONNECTED,
            info.constraints.requiredNetworkType,
        )
    }

    @Test
    fun enqueue_is_idempotent_keeping_a_single_pending_job() {
        SyncScheduler.enqueue(context())
        SyncScheduler.enqueue(context())

        val infos = WorkManager.getInstance(context())
            .getWorkInfosForUniqueWork(SyncScheduler.UNIQUE_WORK_NAME)
            .get()
        assertEquals("KEEP collapses repeat triggers into one pending job", 1, infos.size)
    }
}
