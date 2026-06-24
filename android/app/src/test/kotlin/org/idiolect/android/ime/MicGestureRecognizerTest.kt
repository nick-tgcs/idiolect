package org.idiolect.android.ime

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The mic's gesture grammar, decided with the user:
 *   • press-and-hold (held past a short threshold) = one utterance — release inserts;
 *   • double-tap = continuous mode;
 *   • a lone quick tap = a one-shot toggle (start/stop).
 *
 * The recognizer is pure: it takes down/up/cancel edges plus a [GestureClock] (a fake here,
 * a Handler in the app) so the hold and double-tap timers are deterministic in tests.
 */
class MicGestureRecognizerTest {
    /** Records which gestures fired, in order. */
    private class Recorder : MicGestures {
        val calls = mutableListOf<String>()
        override fun onHoldStart() { calls += "holdStart" }
        override fun onHoldEnd() { calls += "holdEnd" }
        override fun onSingleTap() { calls += "tap" }
        override fun onDoubleTap() { calls += "doubleTap" }
    }

    /** A clock that just remembers scheduled actions so the test fires them by token. */
    private class FakeClock : GestureClock {
        private val scheduled = LinkedHashMap<Any, () -> Unit>()
        override fun postDelayed(delayMs: Long, token: Any, action: () -> Unit) {
            scheduled[token] = action
        }
        override fun remove(token: Any) { scheduled.remove(token) }
        fun fire(token: Any) {
            val action = scheduled.remove(token) ?: error("nothing scheduled for $token")
            action()
        }
        fun isScheduled(token: Any) = scheduled.containsKey(token)
    }

    private val rec = Recorder()
    private val clock = FakeClock()
    private val sut = MicGestureRecognizer(rec, clock)

    @Test
    fun a_lone_quick_tap_is_a_single_tap() {
        sut.onDown()
        sut.onUp() // released before the hold timer → a tap
        clock.fire(MicGestureRecognizer.TAP_TOKEN) // no second tap arrived
        assertEquals(listOf("tap"), rec.calls)
    }

    @Test
    fun two_quick_taps_are_a_double_tap_and_never_a_single_tap() {
        sut.onDown(); sut.onUp() // first tap (arms the double-tap window)
        sut.onDown(); sut.onUp() // second tap within the window
        assertEquals(listOf("doubleTap"), rec.calls)
        // the pending single-tap confirmation must have been cancelled
        assertEquals(false, clock.isScheduled(MicGestureRecognizer.TAP_TOKEN))
    }

    @Test
    fun press_and_hold_then_release_is_a_held_utterance() {
        sut.onDown()
        clock.fire(MicGestureRecognizer.HOLD_TOKEN) // held past the threshold
        sut.onUp()
        assertEquals(listOf("holdStart", "holdEnd"), rec.calls)
    }

    @Test
    fun a_hold_does_not_also_register_a_tap() {
        sut.onDown()
        clock.fire(MicGestureRecognizer.HOLD_TOKEN)
        sut.onUp()
        // no single-tap confirmation should be pending after a hold
        assertEquals(false, clock.isScheduled(MicGestureRecognizer.TAP_TOKEN))
    }

    @Test
    fun releasing_before_the_hold_threshold_never_starts_a_hold() {
        sut.onDown()
        sut.onUp() // up before HOLD_TOKEN fires
        clock.fire(MicGestureRecognizer.TAP_TOKEN)
        assertEquals(listOf("tap"), rec.calls) // a tap, not a hold
    }

    @Test
    fun cancelling_during_a_hold_ends_the_hold() {
        sut.onDown()
        clock.fire(MicGestureRecognizer.HOLD_TOKEN)
        sut.onCancel()
        assertEquals(listOf("holdStart", "holdEnd"), rec.calls)
    }
}
