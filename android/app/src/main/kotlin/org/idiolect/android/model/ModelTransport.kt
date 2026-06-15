package org.idiolect.android.model

import java.io.OutputStream

/**
 * The network seam for fetching a model from the user's PC. The real implementation
 * ([HttpModelTransport]) talks to `idiolect-sync-server`; tests substitute a fake, so
 * the [ModelDownloader] orchestration (resume + verify + atomic install) is host-tested
 * without a server.
 */
interface ModelTransport {
    /** Fetch `GET /v1/model/manifest` — the model's id, digest, and size. */
    fun fetchManifest(): ModelManifest

    /**
     * Download the model bytes starting at [offset] (0 = whole file; >0 sends an HTTP
     * Range to resume), appending them to [sink]. [onBytes] reports the running count of
     * bytes written **in this call** for progress.
     */
    fun download(offset: Long, sink: OutputStream, onBytes: (Long) -> Unit)
}
