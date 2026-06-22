package org.idiolect.android.setup

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * Integration guard for pairing-by-link: the manifest must route a browsable VIEW
 * `idiolect://pair?…` intent to [SetupActivity], so a tapped/fired pairing link reaches
 * enrolment with **no camera**. Mirrors [org.idiolect.android.sync.CameraPermissionTest] — a
 * cheap on-manifest assertion so a future edit can't silently drop the deep link. The pairing
 * the routed intent then performs is covered end-to-end on a device by `PairingDeepLinkE2eTest`.
 */
@RunWith(RobolectricTestRunner::class)
class PairingDeepLinkManifestTest {
    @Test
    fun the_pairing_deep_link_resolves_to_setup_activity() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val intent = Intent(
            Intent.ACTION_VIEW,
            Uri.parse("idiolect://pair?u=http%3A%2F%2F10.0.2.2%3A8765&c=ABCD1234"),
        ).addCategory(Intent.CATEGORY_BROWSABLE)

        val resolved = context.packageManager.resolveActivity(intent, 0)

        assertNotNull("no activity handles the idiolect://pair deep link", resolved)
        assertEquals(SetupActivity::class.java.name, resolved!!.activityInfo.name)
    }
}
