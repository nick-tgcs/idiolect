package org.idiolect.android.sync

import android.content.Context
import android.content.pm.ApplicationInfo
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * Pairing and every sync hop default to **pinned HTTPS** ([CertPin] / [applyPinning]), but the
 * sync server's optional cleartext mode (run with `--no-tls`, for deployments already inside a
 * tailnet or a localhost onion tunnel) speaks plain `http` to the user's *own* machine —
 * `10.0.2.2` from the emulator, a LAN/tailnet IP in the field. On `targetSdk >= 28` Android
 * blocks cleartext by default, which would silently kill [HttpPairingTransport],
 * [HttpSyncTransport] and [model pulls][org.idiolect.android.model.HttpModelTransport] against
 * a `--no-tls` server (the connection throws before it reaches the PC). This guards the manifest
 * opt-in so a future edit can't re-break that fallback. The public model download is *not*
 * loosened by this — PublicModelTransport enforces https in code, independent of this flag.
 */
@RunWith(RobolectricTestRunner::class)
class CleartextPairingTest {
    @Test
    fun cleartext_http_to_the_users_pc_is_permitted_for_the_no_tls_fallback() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val info = context.applicationInfo
        assertTrue(
            "cleartext http must stay permitted or the --no-tls PC fallback is dead on device",
            (info.flags and ApplicationInfo.FLAG_USES_CLEARTEXT_TRAFFIC) != 0,
        )
    }
}
