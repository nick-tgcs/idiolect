package org.idiolect.android.model

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Test

class ModelManifestTest {
    @Test
    fun parses_the_server_manifest_shape() {
        val json = """{"id":"base.en","sha256":"abc123","size":59876543}"""
        assertEquals(ModelManifest("base.en", "abc123", 59_876_543L), ModelManifest.parse(json))
    }

    @Test
    fun tolerates_whitespace_and_field_order() {
        val json = """{ "size" : 10 , "sha256": "ff00" ,  "id" :  "small.en" }"""
        assertEquals(ModelManifest("small.en", "ff00", 10L), ModelManifest.parse(json))
    }

    @Test
    fun rejects_a_manifest_missing_a_field() {
        assertThrows(IllegalArgumentException::class.java) {
            ModelManifest.parse("""{"id":"base.en","sha256":"abc"}""")
        }
    }
}
