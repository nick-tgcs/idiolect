package org.idiolect.android.audio

import android.app.Notification
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config

/**
 * Integration coverage (Robolectric) for the mic foreground service: starting it must
 * put the service in the foreground with an ongoing notification — that visible
 * notification is the privacy contract for microphone use. Pinned to SDK 33 to avoid
 * Android 14's FGS-type permission enforcement (covered for real by the emulator).
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class MicForegroundServiceTest {
    @Test
    fun starting_the_service_goes_foreground_with_an_ongoing_notification() {
        val controller = Robolectric.buildService(MicForegroundService::class.java).create()
        controller.get().onStartCommand(null, 0, 1)

        val notification: Notification? = shadowOf(controller.get()).lastForegroundNotification
        assertNotNull("startForeground posted a notification", notification)
        assertTrue(
            "the notification is ongoing",
            (notification!!.flags and Notification.FLAG_ONGOING_EVENT) != 0,
        )
    }
}
