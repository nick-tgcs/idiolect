package org.idiolect.android.model

import java.io.FileOutputStream

/**
 * Orchestrates pulling a model onto the device: fetch the manifest, download the bytes
 * (**resuming** from any partial `.bin.part`), verify the SHA-256 against the manifest,
 * and **atomically install** it. A digest mismatch deletes the partial and throws —
 * a corrupt model is never installed. Pure logic over [ModelTransport]/[ModelStore], so
 * it is host-tested with a fake transport.
 */
class ModelDownloader(
    private val transport: ModelTransport,
    private val store: ModelStore,
    private val digestOf: (java.io.File) -> String = Sha256::ofFile,
) {
    /** Download + verify + install, reporting `(downloaded, total)` progress. */
    fun download(onProgress: (downloaded: Long, total: Long) -> Unit = { _, _ -> }): InstalledModel {
        val manifest = transport.fetchManifest()
        val part = store.partFile(manifest.id)
        part.parentFile?.mkdirs()

        val existing = if (part.exists()) part.length() else 0L
        // Resume from a valid partial; restart clean if it is absent or already too big.
        val offset = if (existing in 1..manifest.size) existing else { part.delete(); 0L }

        if (offset < manifest.size) {
            FileOutputStream(part, /* append = */ offset > 0).use { sink ->
                transport.download(offset, sink) { written -> onProgress(offset + written, manifest.size) }
            }
        } else {
            onProgress(manifest.size, manifest.size)
        }

        val actual = digestOf(part)
        if (!actual.equals(manifest.sha256, ignoreCase = true)) {
            part.delete()
            throw ModelIntegrityException(manifest.sha256, actual)
        }
        return store.install(manifest.id, manifest.sha256, part)
    }
}
