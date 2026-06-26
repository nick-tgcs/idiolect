package org.idiolect.android.ui

import android.app.Activity
import android.graphics.Rect
import android.view.View
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

/**
 * Edge-to-edge window handling, shared by every full-screen idiolect activity.
 *
 * On `targetSdk 35` Android draws the app **under** the status and navigation bars (the
 * "edge-to-edge" default that can no longer be opted out of on Android 15+). An activity that
 * does nothing has its top row of content hidden behind the clock/status bar — the bug behind
 * "the settings screen cuts off the top". The fix is to consume the [WindowInsetsCompat] and
 * pad the scroll root by the system-bar insets so the content sits in the safe area while the
 * bars stay transparent over the app's own dark background.
 *
 * We *opt into* edge-to-edge explicitly ([enable]) rather than relying on the platform default,
 * so the behaviour — and this padding — is identical on every API level (incl. the API 33
 * emulator the e2e runs on), not just on Android 15 hardware.
 *
 * [paddingFor] is the pure inset arithmetic (unit-tested); [padSystemBars] is the thin view
 * wiring (Robolectric-tested); the rendered result is verified by the emulator screenshots.
 */
object EdgeToEdge {
    /**
     * The padding a root view needs so the [systemBars] insets do not occlude its content,
     * preserving the view's original [base] padding (added to, never replaced).
     */
    fun paddingFor(base: Rect, systemBars: Insets): Rect = Rect(
        base.left + systemBars.left,
        base.top + systemBars.top,
        base.right + systemBars.right,
        base.bottom + systemBars.bottom,
    )

    /**
     * Pad [root] by the system-bar insets on every inset change, keeping its initial padding.
     * Captures the view's starting padding once so repeated dispatches stay idempotent.
     */
    fun padSystemBars(root: View) {
        val base = Rect(root.paddingLeft, root.paddingTop, root.paddingRight, root.paddingBottom)
        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
            val padded = paddingFor(base, bars)
            view.setPadding(padded.left, padded.top, padded.right, padded.bottom)
            WindowInsetsCompat.CONSUMED
        }
    }

    /**
     * Opt [activity] into edge-to-edge and inset [scrollRoot] for the system bars. The status-bar
     * icons are drawn light (the app background is always dark slate), so the clock stays legible.
     */
    fun enable(activity: Activity, scrollRoot: View) {
        WindowCompat.setDecorFitsSystemWindows(activity.window, false)
        WindowCompat.getInsetsController(activity.window, activity.window.decorView)
            .isAppearanceLightStatusBars = false
        padSystemBars(scrollRoot)
    }
}
