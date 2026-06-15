package org.idiolect.android.model

import java.io.File

/**
 * The on-device model store: model files live under [root] (the app's private
 * `filesDir/models/whisper`) as `{id}.bin`, with a `.bin.part` companion for an
 * in-progress (resumable) download. Installs are **atomic** (rename the verified temp
 * over the destination) so a crash mid-write never leaves a half-model that would fail
 * the load-time integrity check anyway. The active model's id + digest are recorded so
 * the IME can lazily verify-and-load it on first focus.
 */
class ModelStore(private val root: File) {
    fun modelFile(id: String): File = File(root, "$id.bin")

    fun partFile(id: String): File = File(root, "$id.bin.part")

    fun isInstalled(id: String): Boolean = modelFile(id).exists()

    /** Atomically place the verified [temp] file as model [id] and record it active. */
    fun install(id: String, sha256: String, temp: File): InstalledModel {
        root.mkdirs()
        val dest = modelFile(id)
        dest.delete()
        if (!temp.renameTo(dest)) {
            // Cross-filesystem fallback (rename can fail across mounts).
            temp.copyTo(dest, overwrite = true)
            temp.delete()
        }
        val model = InstalledModel(id, sha256, dest.absolutePath)
        activeFile().writeText("$id\n$sha256")
        return model
    }

    /** The active installed model, or `null` if none is installed (or its file is gone). */
    fun active(): InstalledModel? {
        val lines = activeFile().takeIf { it.exists() }?.readLines() ?: return null
        if (lines.size < 2) return null
        val (id, sha256) = lines
        val file = modelFile(id)
        return if (file.exists()) InstalledModel(id, sha256, file.absolutePath) else null
    }

    private fun activeFile(): File = File(root, "active")
}
