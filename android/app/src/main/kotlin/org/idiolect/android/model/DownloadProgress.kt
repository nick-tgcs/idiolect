package org.idiolect.android.model

import java.util.Locale

/**
 * Formats a model-download progress line as moving megabytes + a clamped percent, e.g.
 * "12.3 / 30.7 MB · 40%". Visible byte movement reassures the user the download is alive (the
 * old bare-percent line read as frozen on a slow phone link). Megabytes are MiB, matching the
 * catalog's "31 MB" size hints. Pure → unit-tested; shared by onboarding and settings.
 */
object DownloadProgress {
    private const val MIB = 1L shl 20

    fun label(downloaded: Long, total: Long): String {
        val mb = downloaded.toDouble() / MIB
        if (total <= 0) return String.format(Locale.US, "%.1f MB", mb)
        val totalMb = total.toDouble() / MIB
        val percent = ((downloaded * 100 / total).coerceIn(0, 100)).toInt()
        return String.format(Locale.US, "%.1f / %.1f MB · %d%%", mb, totalMb, percent)
    }
}
