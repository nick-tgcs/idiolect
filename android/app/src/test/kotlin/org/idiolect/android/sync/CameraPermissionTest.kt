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
 * Guards the permission the QR-pairing flow depends on: without `CAMERA`, the zxing
 * scanner launched from setup can't open the lens, so a user could never pair by scanning.
 * Cheap manifest assertion (mirrors [InternetPermissionTest]) so a future manifest edit
 * can't silently break enrolment-by-scan. The camera stays *optional* at the feature level
 * (see the `uses-feature ... required="false"`), so this permission never blocks install on
 * a camera-less device — the typed-code fallback still pairs there.
 */
@RunWith(RobolectricTestRunner::class)
class CameraPermissionTest {
    @Test
    fun the_app_declares_camera_for_qr_pairing() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val info = context.packageManager
            .getPackageInfo(context.packageName, PackageManager.GET_PERMISSIONS)
        assertTrue(
            "scanning the PC's pairing QR needs CAMERA",
            info.requestedPermissions?.contains(Manifest.permission.CAMERA) == true,
        )
    }
}
