package org.idiolect.android.model

import java.io.File
import java.io.IOException

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
        // Atomic replace — never delete the existing model first. rename() overwrites the
        // destination in a single step (same filesystem), so the old model stays fully in
        // place until the new file is durably there; a crash in this window can never leave
        // `active` pointing at a missing or half-written model.
        if (!temp.renameTo(dest)) {
            // rename can't span mounts: stage a full copy ON the destination's filesystem,
            // then atomically swap it in. The old model is untouched until that final rename.
            val staging = File(dest.parentFile, "$id.bin.staging")
            temp.copyTo(staging, overwrite = true)
            if (!staging.renameTo(dest)) {
                staging.delete()
                throw IOException("could not install model $id: atomic replace failed")
            }
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
