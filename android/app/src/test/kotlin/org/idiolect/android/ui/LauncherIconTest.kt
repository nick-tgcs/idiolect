package org.idiolect.android.ui

import android.content.Context
import androidx.core.content.ContextCompat
import androidx.test.core.app.ApplicationProvider
import org.idiolect.android.R
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNotNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * Guards that the app ships its own launcher icon. With no `android:icon` the launcher and
 * the Obtainium install screen fall back to the stock green-robot placeholder (the "no proper
 * icons" bug). This asserts the manifest declares the icon and that it resolves to a real
 * drawable (the adaptive mic-on-periwinkle mark).
 */
@RunWith(RobolectricTestRunner::class)
class LauncherIconTest {
    @Test
    fun the_app_declares_its_own_launcher_icon() {
        val context = ApplicationProvider.getApplicationContext<Context>()
        val icon = context.applicationInfo.icon
        assertNotEquals("the app must declare android:icon, not fall back to the system default", 0, icon)
        assertEquals("the launcher icon is the idiolect mark", R.mipmap.ic_launcher, icon)
        assertNotNull("the declared icon resolves to a real drawable", ContextCompat.getDrawable(context, icon))
    }
}
