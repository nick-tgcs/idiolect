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
    /**
     * Download + verify + install, reporting `(downloaded, total)` progress. [isCancelled] is
     * polled once the bytes verify (a cheap early-out before the copy) and again at install's
     * commit point (after the expensive copy, before the model is published active): if the
     * caller has cancelled (or a newer download has superseded this one) — even mid-copy — the
     * model is left uninstalled and [ModelDownloadCancelledException] is thrown, so an abandoned
     * download can never advance onboarding to Ready. A cancel *before* install keeps the
     * verified `.part` so a retry resumes instantly; a cancel *during* the commit discards the
     * staged bytes, so that retry re-downloads.
     */
    fun download(
        isCancelled: () -> Boolean = { false },
        onProgress: (downloaded: Long, total: Long) -> Unit = { _, _ -> },
    ): InstalledModel {
        val manifest = transport.fetchManifest()
        val part = store.partFile(manifest.id)
        part.parentFile?.mkdirs()

        // A first pass may resume from a partial. If a *resumed* pass fails its digest it is
        // almost always because the server ignored our Range and restreamed from byte 0,
        // appending onto the stale partial — so discard and retry once cleanly from 0. This
        // turns a recoverable dropped-resume into a transparent retry rather than surfacing
        // a scary integrity error and a wasted re-download to the user. A *fresh* (offset 0)
        // mismatch is genuine corruption or a bad pin, so it is not retried.
        val resumed = part.exists() && part.length() in 1..manifest.size
        fetchInto(manifest, part, onProgress)
        var actual = digestOf(part)
        if (!actual.equals(manifest.sha256, ignoreCase = true) && resumed) {
            part.delete()
            fetchInto(manifest, part, onProgress)
            actual = digestOf(part)
        }
        if (!actual.equals(manifest.sha256, ignoreCase = true)) {
            part.delete()
            throw ModelIntegrityException(manifest.sha256, actual)
        }
        // Cheap early-out before the expensive copy: skip install entirely if already cancelled.
        // The commit inside install is gated on the same signal, so a cancel that lands *during*
        // the copy still publishes nothing (see ModelStore.install).
        if (isCancelled()) throw ModelDownloadCancelledException()
        return store.install(manifest.id, manifest.sha256, part, isCancelled)
    }

    /** Stream the model into [part], resuming from its current length if it is a valid partial. */
    private fun fetchInto(manifest: ModelManifest, part: java.io.File, onProgress: (Long, Long) -> Unit) {
        val existing = if (part.exists()) part.length() else 0L
        // Resume from a valid partial; restart clean if it is absent or already too big.
        val offset = if (existing in 1..manifest.size) existing else { part.delete(); 0L }
        if (offset < manifest.size) {
            FileOutputStream(part, /* append = */ offset > 0).use { sink ->
                transport.download(offset, sink) { written ->
                    // Clamp: a Range-ignoring server restreams past `offset`, which would
                    // otherwise report >100%.
                    onProgress(minOf(offset + written, manifest.size), manifest.size)
                }
            }
        } else {
            onProgress(manifest.size, manifest.size)
        }
    }
}
