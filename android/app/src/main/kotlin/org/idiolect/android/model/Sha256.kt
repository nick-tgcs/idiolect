package org.idiolect.android.model

import java.io.File
import java.security.MessageDigest

/**
 * Streaming SHA-256 of a file as lowercase hex — the Kotlin counterpart of the Rust
 * `idiolect_common::digest::file_sha256_hex`. Used to verify a downloaded model before
 * install; the Rust core re-verifies the same digest at every load (M5a). Both compute
 * standard SHA-256 lowercase hex, so they agree byte-for-byte.
 */
object Sha256 {
    fun ofFile(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { input ->
            val buffer = ByteArray(64 * 1024)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it.toInt() and 0xFF) }
    }
}
