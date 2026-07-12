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

    /**
     * Atomically place the verified [temp] file as model [id] and record it active. The
     * expensive placement (a rename, or a full copy across mounts) writes a private
     * `.bin.staging` file that touches neither `{id}.bin` nor `active`; [isCancelled] is then
     * polled once, and only on a clear does the fast commit run — swap staging over the
     * destination, then write the active marker. A cancel/supersede landing *during* the copy
     * therefore discards the staging file and throws [ModelDownloadCancelledException] without
     * ever publishing the model, so a late/superseded download can never win the `active`
     * marker behind a suppressed UI.
     */
    fun install(id: String, sha256: String, temp: File, isCancelled: () -> Boolean = { false }): InstalledModel {
        root.mkdirs()
        val dest = modelFile(id)
        // Stage the bytes onto the destination's filesystem first. This is the slow part (a
        // cross-mount copy) and it touches neither `dest` nor `active`, so a cancel during it
        // publishes nothing. A same-filesystem rename is the fast path; the copy is the mount
        // fallback.
        val staging = File(dest.parentFile, "$id.bin.staging")
        if (!temp.renameTo(staging)) {
            temp.copyTo(staging, overwrite = true)
            temp.delete()
        }
        // Commit gate: a cancel/supersede that landed while we were copying stops here, leaving
        // the old model in place and nothing new published.
        if (isCancelled()) {
            staging.delete()
            throw ModelDownloadCancelledException()
        }
        // Atomic replace — never delete the existing model first. rename() overwrites the
        // destination in a single step (same filesystem), so the old model stays fully in place
        // until the new file is durably there; a crash in this window can never leave `active`
        // pointing at a missing or half-written model.
        if (!staging.renameTo(dest)) {
            staging.delete()
            throw IOException("could not install model $id: atomic replace failed")
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
