package org.idiolect.android.settings

import java.io.File
import java.util.Locale
import kotlin.math.roundToLong

/**
 * Reports the captured-audio footprint for the "Audio on device" row. The Rust core writes
 * source-audio blobs under `filesDir/audio` (date-nested, see `RootedPaths.audio_dir`) and
 * enforces the 1 GiB retention cap itself, evicting the oldest; this just *sums* what is on
 * disk and formats it against the cap, so no FFI round-trip is needed to show the figure.
 */
object AudioUsage {
    private const val KB = 1024.0
    private const val MB = KB * 1024
    private const val GB = MB * 1024

    /** Total bytes under [audioDir], walked recursively. An absent store is 0 (nothing captured). */
    fun bytesOnDisk(audioDir: File): Long {
        if (!audioDir.exists()) return 0L
        return audioDir.walkTopDown().filter { it.isFile }.sumOf { it.length() }
    }

    /** "214 MB of 1.0 GB" — the footprint against the retention cap. */
    fun format(usedBytes: Long, capBytes: Long): String =
        "${humanize(usedBytes)} of ${humanize(capBytes)}"

    /** Binary units (labelled MB/GB, as phone storage UIs do); GB carries one decimal. */
    private fun humanize(bytes: Long): String = when {
        bytes >= GB -> String.format(Locale.US, "%.1f GB", bytes / GB)
        bytes >= MB -> "${(bytes / MB).roundToLong()} MB"
        bytes >= KB -> "${(bytes / KB).roundToLong()} KB"
        else -> "$bytes B"
    }
}
