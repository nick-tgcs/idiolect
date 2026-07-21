package org.idiolect.android.ui

import android.content.Context
import androidx.test.core.app.ApplicationProvider
import org.idiolect.android.R
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * Guards that the app ships its own dark, no-ActionBar theme. Without a declared theme the app
 * inherits the platform default, which (a) draws a framework ActionBar over the screens that
 * paint their own header, and (b) on Android 15 leaves the bars opaque/awkward. [Theme_Idiolect]
 * is the dark slate, transparent-bar, no-ActionBar theme the edge-to-edge handling assumes.
 */
@RunWith(RobolectricTestRunner::class)
class AppThemeTest {
    @Test
    fun the_application_declares_the_idiolect_theme() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        assertEquals(
            "the <application> must use Theme.Idiolect (dark, no ActionBar)",
            R.style.Theme_Idiolect,
            context.applicationInfo.theme,
        )
    }
}
