package org.idiolect.android.ime

/** The gestures the mic recognises (see [MicGestureRecognizer] for the grammar). */
interface MicGestures {
    /** A press has been held past the threshold — begin a press-to-talk utterance. */
    fun onHoldStart()

    /** The held press was released (or cancelled) — finalize the one utterance. */
    fun onHoldEnd()

    /** A lone quick tap — toggle a one-shot take (start if idle, stop if listening). */
    fun onSingleTap()

    /** Two quick taps — toggle continuous mode. */
    fun onDoubleTap()
}

/**
 * Schedules the hold and double-tap timers. Backed by a `Handler` in the app and a fake in
 * tests, so the recognizer's timing is deterministic without a real clock.
 */
interface GestureClock {
    fun postDelayed(delayMs: Long, token: Any, action: () -> Unit)
    fun remove(token: Any)
}

/**
 * Turns raw down/up/cancel touch edges into the mic's gesture grammar (decided with the user):
 *
 *   • **press-and-hold** — held past [holdStartMs] → [MicGestures.onHoldStart], release →
 *     [MicGestures.onHoldEnd]. Press-to-talk: one utterance per hold.
 *   • **double-tap** — two quick taps within [doubleTapMs] → [MicGestures.onDoubleTap].
 *   • **lone tap** — a quick tap with no follow-up → [MicGestures.onSingleTap] (a one-shot
 *     toggle). Confirmed only after the double-tap window passes, so it never fires as half
 *     of a double-tap.
 *
 * Pure and single-threaded (the app drives it from the main thread); all timing goes through
 * [GestureClock] so tests stay deterministic.
 */
class MicGestureRecognizer(
    private val gestures: MicGestures,
    private val clock: GestureClock,
    private val holdStartMs: Long = DEFAULT_HOLD_START_MS,
    private val doubleTapMs: Long = DEFAULT_DOUBLE_TAP_MS,
) {
    private var holdStarted = false
    private var awaitingSecondTap = false

    fun onDown() {
        // A new press supersedes a pending single-tap confirmation (it may be a double-tap
        // or a hold). The double-tap intent is remembered in [awaitingSecondTap].
        clock.remove(TAP_TOKEN)
        holdStarted = false
        clock.postDelayed(holdStartMs, HOLD_TOKEN) {
            holdStarted = true
            gestures.onHoldStart()
        }
    }

    fun onUp() {
        clock.remove(HOLD_TOKEN)
        if (holdStarted) {
            holdStarted = false
            awaitingSecondTap = false
            gestures.onHoldEnd()
            return
        }
        // A quick tap (released before the hold threshold).
        if (awaitingSecondTap) {
            awaitingSecondTap = false
            gestures.onDoubleTap()
        } else {
            awaitingSecondTap = true
            clock.postDelayed(doubleTapMs, TAP_TOKEN) {
                awaitingSecondTap = false
                gestures.onSingleTap()
            }
        }
    }

    fun onCancel() {
        clock.remove(HOLD_TOKEN)
        clock.remove(TAP_TOKEN)
        if (holdStarted) {
            holdStarted = false
            gestures.onHoldEnd()
        }
        awaitingSecondTap = false
    }

    companion object {
        const val DEFAULT_HOLD_START_MS = 160L
        const val DEFAULT_DOUBLE_TAP_MS = 260L

        /** Timer tokens (also used by tests to fire the timers deterministically). */
        val HOLD_TOKEN = Any()
        val TAP_TOKEN = Any()
    }
}
