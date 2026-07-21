package org.idiolect.android.settings

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.nio.file.Files

/**
 * The persisted toggle store behind the settings screen, written as a single `key=value`
 * file under the app's private filesDir (the same plain-file pattern as `SecureSyncConfig` /
 * `ModelStore`, so it is host-tested with a temp dir — no SharedPreferences / Robolectric).
 * Every flag has a default matching the pre-settings behaviour, so an un-written store reads
 * as today's behaviour and the screen never silently changes how dictation already worked.
 */
class SettingsStoreTest {
    private fun newStore(): SettingsStore {
        val dir = Files.createTempDirectory("settings-store").toFile()
        return SettingsStore(File(dir, SettingsStore.FILE_NAME))
    }

    @Test
    fun defaults_match_pre_settings_behaviour() {
        val store = newStore()
        // Review was off by default (the 👁 strip toggle started unlit); double-tap always
        // entered continuous; corrections always shipped to a paired PC.
        assertFalse("review defaults off, as the strip toggle did", store.reviewByDefault())
        assertTrue("double-tap-for-continuous defaults on", store.continuousOnDoubleTap())
        assertTrue("shipping corrections defaults on", store.shipCorrections())
    }

    @Test
    fun review_round_trips() {
        val store = newStore()
        store.setReviewByDefault(true)
        assertTrue(store.reviewByDefault())
        store.setReviewByDefault(false)
        assertFalse(store.reviewByDefault())
    }

    @Test
    fun continuous_round_trips() {
        val store = newStore()
        store.setContinuousOnDoubleTap(false)
        assertFalse(store.continuousOnDoubleTap())
    }

    @Test
    fun ship_round_trips() {
        val store = newStore()
        store.setShipCorrections(false)
        assertFalse(store.shipCorrections())
    }

    @Test
    fun quick_launch_defaults_on_so_the_floating_mic_actually_dictates() {
        // The floating accessibility button used to do nothing; with this on, a tap dictates.
        assertTrue("quick-launch mic defaults on", newStore().quickLaunchEnabled())
    }

    @Test
    fun quick_launch_round_trips() {
        val store = newStore()
        store.setQuickLaunchEnabled(false)
        assertFalse(store.quickLaunchEnabled())
        store.setQuickLaunchEnabled(true)
        assertTrue(store.quickLaunchEnabled())
    }

    @Test
    fun flags_are_independent() {
        val store = newStore()
        store.setReviewByDefault(true)
        store.setContinuousOnDoubleTap(false)
        // The untouched flag keeps its default rather than being clobbered by a sibling write.
        assertTrue("ship stays at its default while other flags change", store.shipCorrections())
        assertTrue(store.reviewByDefault())
        assertFalse(store.continuousOnDoubleTap())
    }

    @Test
    fun values_persist_across_a_reopen() {
        val dir = Files.createTempDirectory("settings-store").toFile()
        val file = File(dir, SettingsStore.FILE_NAME)
        SettingsStore(file).setReviewByDefault(true)
        // A fresh instance over the same file (a process restart) sees the persisted value.
        assertEquals(true, SettingsStore(file).reviewByDefault())
    }
}
