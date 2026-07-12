package org.idiolect.android

import org.idiolect.android.settings.SettingsActivity
import org.idiolect.android.setup.SetupActivity
import org.junit.Assert.assertTrue
import org.junit.Test
import java.lang.reflect.Modifier

/**
 * The model-download cancel token is written on the UI thread (a new/cancelled download bumps it)
 * and read on the daemon download thread inside the `isCancelled` gate that decides whether
 * `ModelStore.install` may publish. With a plain `var` there is no happens-before edge, so the
 * download thread can observe a stale token and publish a model the user already cancelled or
 * superseded (Codex P2 on PR #67). A true race is nondeterministic to test, so we pin the exact
 * mechanism that provides the visibility guarantee: the backing field must be JVM-`volatile`
 * (Kotlin `@Volatile`). This regresses the moment someone drops the annotation.
 */
class DownloadCancelTokenVisibilityTest {
    private fun assertVolatile(cls: Class<*>, field: String) {
        val f = cls.getDeclaredField(field)
        assertTrue(
            "$field must be @Volatile so the download thread observes cancels/supersedes",
            Modifier.isVolatile(f.modifiers),
        )
    }

    @Test
    fun setup_download_token_is_volatile() =
        assertVolatile(SetupActivity::class.java, "downloadToken")

    @Test
    fun settings_download_token_is_volatile() =
        assertVolatile(SettingsActivity::class.java, "modelDownloadToken")
}
