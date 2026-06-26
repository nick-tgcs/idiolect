package org.idiolect.android.model

import java.io.OutputStream
import java.net.HttpURLConnection
import java.net.URL

/**
 * [ModelTransport] for the zero-config, PC-less path: download a base speech model from a
 * fixed PUBLIC url (the whisper.cpp ggml release on Hugging Face) with NO authentication,
 * verified against a digest [pinned] into the app at build time.
 *
 * It differs from [HttpModelTransport] in three ways, all because the source is a public
 * CDN rather than the user's `idiolect-sync-server`: there is no server-served manifest
 * (the CDN returns only raw bytes, so the manifest is supplied here and [ModelDownloader]
 * checks the bytes against it exactly as for the PC path); no bearer token is sent; and
 * the url must be **https** (or loopback, for host tests).
 *
 * The pinned SHA-256 in [ModelDownloader] is the sole integrity gate — it rejects any
 * tampered or wrong bytes no matter where a redirect leads. Requiring https is defence in
 * depth on top of it: it keeps an observer from seeing which model is pulled and an active
 * attacker from wasting bandwidth, and because HttpURLConnection refuses a cross-protocol
 * redirect, a 302 can't quietly downgrade the HF→CDN hop onto cleartext.
 */
class PublicModelTransport(
    private val url: String,
    private val pinned: ModelManifest,
    private val connectTimeoutMs: Int = 15_000,
    private val readTimeoutMs: Int = 120_000,
) : ModelTransport {
    init {
        val parsed = URL(url)
        // URL.host keeps IPv6 brackets ("[::1]"); strip them before the loopback compare.
        val host = parsed.host.removeSurrounding("[", "]").lowercase()
        val loopback = host == "127.0.0.1" || host == "::1" || host == "localhost"
        require(parsed.protocol.equals("https", ignoreCase = true) || loopback) {
            "public model url must use https (got: $url)"
        }
    }

    override fun fetchManifest(): ModelManifest = pinned

    override fun download(offset: Long, sink: OutputStream, onBytes: (Long) -> Unit) {
        val connection = URL(url).openConnection() as HttpURLConnection
        connection.connectTimeout = connectTimeoutMs
        connection.readTimeout = readTimeoutMs
        connection.instanceFollowRedirects = true // the HF resolve url 302-redirects to its CDN
        if (offset > 0) connection.setRequestProperty("Range", "bytes=$offset-")
        try {
            val code = connection.responseCode
            // 206 = our Range was honored (resume). 200 = the CDN ignored Range and is
            // restreaming from 0; ModelDownloader recovers by retrying cleanly from 0, so
            // accept both here. Anything else (incl. an unfollowed cross-protocol 3xx)
            // fails the download rather than streaming untrusted/cleartext bytes.
            require(code == HttpURLConnection.HTTP_OK || code == HttpURLConnection.HTTP_PARTIAL) {
                "public model download failed: HTTP $code"
            }
            connection.inputStream.use { input ->
                val buffer = ByteArray(64 * 1024)
                var total = 0L
                while (true) {
                    val read = input.read(buffer)
                    if (read < 0) break
                    sink.write(buffer, 0, read)
                    total += read
                    onBytes(total)
                }
            }
        } finally {
            connection.disconnect()
        }
    }

    companion object {
        /**
         * The recommended zero-config model source for a phone that never sees a PC — the
         * [PublicModelCatalog] default (currently the fast tiny.en q5_1). The catalog is the
         * single source of truth for which models exist and which is the default; this is a
         * convenience for callers that just want "the default download".
         */
        fun recommended(): PublicModelTransport = PublicModelCatalog.default.transport()
    }
}
