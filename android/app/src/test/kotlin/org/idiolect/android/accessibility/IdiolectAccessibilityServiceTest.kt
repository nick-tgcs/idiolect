package org.idiolect.android.accessibility

import org.idiolect.android.recognition.FakeRecognitionTake
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config

/**
 * Teardown of a live quick-launch take. The service can be destroyed (user toggles the a11y
 * service off, system reclaims it) while a take is still listening; releasing without cancelling
 * leaves the mic capture running with no owner until the process dies, so the destroy path must
 * cancel first. The node/injection wiring has no headless seam (covered by the connected e2e);
 * this covers the take lifecycle around it. Pinned to SDK 33 to match the other service tests.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [33])
class IdiolectAccessibilityServiceTest {
    @Test
    fun destroying_the_service_mid_quick_take_cancels_capture_before_releasing() {
        val service = Robolectric.buildService(IdiolectAccessibilityService::class.java).create().get()
        val take = FakeRecognitionTake()
        service.quickTake = take

        service.onDestroy()

        // release() alone drops the core reference but does NOT stop a still-listening
        // capture (see RecognizeSpeechActivity.onDestroy, which cancels for this reason).
        assertEquals(listOf("cancel", "release"), take.calls)
        assertNull("the finished take must be dropped", service.quickTake)
    }
}
