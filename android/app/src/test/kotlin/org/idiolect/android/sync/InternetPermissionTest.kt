package org.idiolect.android.sync

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * Guards the one permission the whole sync (and model-download) story depends on:
 * without `INTERNET`, `HttpURLConnection` throws `SecurityException` on-device, so the
 * outbox could never ship. Cheap manifest assertion so a future manifest edit can't
 * silently break network egress.
 */
@RunWith(RobolectricTestRunner::class)
class InternetPermissionTest {
    @Test
    fun the_app_declares_internet_for_sync_and_model_http() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val info = context.packageManager
            .getPackageInfo(context.packageName, PackageManager.GET_PERMISSIONS)
        assertTrue(
            "POSTing the outbox + downloading the model need INTERNET",
            info.requestedPermissions?.contains(Manifest.permission.INTERNET) == true,
        )
    }
}
