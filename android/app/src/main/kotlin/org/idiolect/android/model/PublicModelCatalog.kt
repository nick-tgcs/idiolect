package org.idiolect.android.model

/**
 * The catalog of selectable on-device speech models for the zero-config, PC-less path. Each
 * [PublicModelOption] pins the public download URL and its integrity manifest (the SHA-256 is
 * the sole trust anchor — see [PublicModelTransport]) and carries the display metadata the
 * onboarding and settings model pickers render.
 *
 * The two offered models are **quantized** (`q5_1`) whisper.cpp English models, a deliberate
 * change from the original full-precision `ggml-base.en` (141 MB f16): q5_1 is a third of the
 * download and markedly faster on a phone CPU at ~the same accuracy. The default is **tiny.en**
 * — several times faster again — so dictation is quick out of the box on any device; base.en is
 * the one-tap upgrade for users who want more accuracy on hard/noisy audio and can spend the
 * extra compute. To change a pin, swap [sha256] + [size] together with the [url] they describe.
 */
data class PublicModelOption(
    val id: String,
    val url: String,
    val sha256: String,
    val size: Long,
    /** Short picker label, e.g. "Tiny (English)". */
    val label: String,
    /** Human download-size hint, e.g. "31 MB". */
    val sizeLabel: String,
    /** One-line speed/accuracy trade-off shown under the label. */
    val blurb: String,
) {
    val manifest: ModelManifest get() = ModelManifest(id, sha256, size)

    /** The pinned, https transport that downloads exactly this model. */
    fun transport(): PublicModelTransport = PublicModelTransport(url, manifest)
}

object PublicModelCatalog {
    /** `ggerganov/whisper.cpp` hosts the ggml models; the digest, not the host, is the trust anchor. */
    private const val BASE_URL = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main"

    /** Fastest, smallest. The default for a phone that never sees a PC. */
    val TINY_EN_Q5_1 = PublicModelOption(
        id = "ggml-tiny.en-q5_1",
        url = "$BASE_URL/ggml-tiny.en-q5_1.bin",
        sha256 = "c77c5766f1cef09b6b7d47f21b546cbddd4157886b3b5d6d4f709e91e66c7c2b",
        size = 32_166_155L,
        label = "Tiny (English)",
        sizeLabel = "31 MB",
        blurb = "Fastest. Best for clear speech — recommended.",
    )

    /** More accurate, slower, larger. The opt-in upgrade. */
    val BASE_EN_Q5_1 = PublicModelOption(
        id = "ggml-base.en-q5_1",
        url = "$BASE_URL/ggml-base.en-q5_1.bin",
        sha256 = "4baf70dd0d7c4247ba2b81fafd9c01005ac77c2f9ef064e00dcf195d0e2fdd2f",
        size = 59_721_011L,
        label = "Base (English)",
        sizeLabel = "57 MB",
        blurb = "More accurate, slower. Better on noisy audio.",
    )

    /** All public options, **default first** so a picker can preselect index 0. */
    val options: List<PublicModelOption> = listOf(TINY_EN_Q5_1, BASE_EN_Q5_1)

    /** The recommended zero-config model for a phone that never sees a PC. */
    val default: PublicModelOption = TINY_EN_Q5_1

    /** The catalog entry for an installed model [id], or null if it is PC-served / unknown. */
    fun byId(id: String): PublicModelOption? = options.firstOrNull { it.id == id }
}
