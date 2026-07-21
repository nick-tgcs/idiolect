package org.idiolect.android.model

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The human-readable model-download progress line. The old onboarding showed only a bare
 * "Downloading… 40%", which on a slow phone download reads as *frozen*; showing the moving
 * megabyte count makes it obviously alive. Pure formatting, so it is unit-tested (and shared by
 * onboarding and settings).
 */
class DownloadProgressTest {
    @Test
    fun shows_moving_megabytes_and_percent() {
        // tiny.en (32,166,155 bytes) at exactly 40%.
        assertEquals("12.3 / 30.7 MB · 40%", DownloadProgress.label(12_866_462, 32_166_155))
    }

    @Test
    fun reports_one_hundred_percent_when_complete() {
        assertEquals("30.7 / 30.7 MB · 100%", DownloadProgress.label(32_166_155, 32_166_155))
    }

    @Test
    fun an_unknown_total_falls_back_to_a_bare_megabyte_count() {
        assertEquals("5.0 MB", DownloadProgress.label(5L shl 20, 0))
    }

    @Test
    fun the_percent_never_exceeds_one_hundred_if_a_server_overstreams() {
        // A Range-ignoring server can push past the expected size; the bar must not read 130%.
        assertEquals("31.5 / 30.7 MB · 100%", DownloadProgress.label(33_000_000, 32_166_155))
    }
}
