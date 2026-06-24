package org.idiolect.android.accessibility

import java.io.File

/**
 * A one-slot, file-backed hand-off for the reviewed correction: the [ReviewActivity] [put]s the
 * approved text, and [IdiolectAccessibilityService] [take]s it when the user's field next
 * regains focus and types it in. It's a file (not an in-memory singleton) because the dialog
 * and the bound accessibility service can live in **different processes** — the pending text
 * must cross that boundary. Single producer, single consumer, so plain read/write/delete is
 * enough; the write is atomic (temp + rename) so a half-written file is never read.
 */
class InjectQueue(private val file: File) {
    /** Stash [text] to be injected on the next host-field focus (overwrites any pending value). */
    fun put(text: String) {
        val tmp = File(file.parentFile, "${file.name}.tmp")
        tmp.writeText(text)
        if (!tmp.renameTo(file)) {
            file.writeText(text) // rename can fail across some FS states; fall back to direct write
            tmp.delete()
        }
    }

    /** Read and clear the pending text, or null if nothing is queued. */
    fun take(): String? {
        if (!file.exists()) return null
        return runCatching { file.readText() }.getOrNull().also { file.delete() }
    }
}
