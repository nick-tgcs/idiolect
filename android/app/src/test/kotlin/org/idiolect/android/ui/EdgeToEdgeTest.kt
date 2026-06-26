package org.idiolect.android.ui

import android.graphics.Rect
import android.view.View
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner

/**
 * The edge-to-edge inset maths and wiring. On `targetSdk 35` the window draws under the
 * system bars (status + navigation), so a screen that does not consume the insets has its
 * top occluded — the bug behind "the settings screen cuts off the top". [EdgeToEdge.paddingFor]
 * is the pure padding calculation (unit-tested here); [EdgeToEdge.padSystemBars] is the thin
 * view wiring (Robolectric — dispatch an inset and assert the view absorbed it as padding).
 */
@RunWith(RobolectricTestRunner::class)
class EdgeToEdgeTest {
    @Test
    fun padding_adds_the_system_bar_insets_to_the_views_own_padding() {
        val base = Rect(16, 40, 16, 28)
        val bars = Insets.of(0, 63, 0, 132) // a status bar on top, a gesture/nav bar below

        val padded = EdgeToEdge.paddingFor(base, bars)

        assertEquals("left keeps its base", 16, padded.left)
        assertEquals("top is base + status bar so content clears it", 40 + 63, padded.top)
        assertEquals("right keeps its base", 16, padded.right)
        assertEquals("bottom is base + nav bar", 28 + 132, padded.bottom)
    }

    @Test
    fun no_insets_leaves_the_base_padding_untouched() {
        val base = Rect(16, 40, 16, 28)
        val padded = EdgeToEdge.paddingFor(base, Insets.NONE)
        assertEquals(Rect(16, 40, 16, 28), padded)
    }

    @Test
    fun pad_system_bars_absorbs_a_dispatched_inset_as_padding() {
        val view = View(ApplicationProvider.getApplicationContext()).apply {
            setPadding(16, 40, 16, 28)
        }
        EdgeToEdge.padSystemBars(view)

        val insets = WindowInsetsCompat.Builder()
            .setInsets(WindowInsetsCompat.Type.systemBars(), Insets.of(0, 63, 0, 132))
            .build()
        ViewCompat.dispatchApplyWindowInsets(view, insets)

        assertEquals("status bar pushes the top padding down", 40 + 63, view.paddingTop)
        assertEquals("nav bar lifts the bottom padding", 28 + 132, view.paddingBottom)
    }
}
