package org.idiolect.android.sync

/**
 * Recognises the `idiolect://pair?u=…&c=…` deep link so a pairing link — tapped, shared, or
 * fired with `adb shell am start -d` — drives the same enrolment path as a scanned QR, with
 * **no camera**. Only a well-formed pairing link is acted on: a plain launch (no data) or any
 * other URL returns `null` and is ignored, so an arbitrary VIEW intent can never be mistaken
 * for a pairing. The url/code are validated downstream by [PairingUri.parse] (to which the
 * recognised string is handed unchanged); this gate only matches the scheme + host + query,
 * sharing [PairingUri]'s exact prefix so the two never disagree.
 *
 * Dependency-free (no `android.net.Uri`) so it is host-testable without Robolectric, like
 * [PairingUri] and [PairingResponse].
 */
object PairingDeepLink {
    /** The scheme://host the pairing link uses; the query carries `u` + `c`. Matches
     *  [PairingUri]'s own prefix so a recognised link always parses. */
    private const val PREFIX = "idiolect://pair?"

    /** The pairing URI string if [dataString] is an idiolect pairing link, else `null`. */
    fun fromIntentData(dataString: String?): String? {
        val data = dataString?.trim() ?: return null
        return if (data.startsWith(PREFIX)) data else null
    }
}
