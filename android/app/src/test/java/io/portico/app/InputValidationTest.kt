package io.portico.app

import io.portico.app.ui.Validation
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class InputValidationTest {

    @Test
    fun vidPidValidationNormalizesAndRejectsMalformed() {
        assertEquals("0123/AE3C", Validation.parseVidPid("0123/ae3c"))
        assertEquals("0781/5583", Validation.parseVidPid("0781:5583"))
        assertEquals("046D/C52B", Validation.parseVidPid("046dc52b"))

        assertNull(Validation.parseVidPid("123/AE3C"))
        assertNull(Validation.parseVidPid("GGGG/1234"))
        assertNull(Validation.parseVidPid(""))
    }

    @Test
    fun formatsAttachedPickerItemPerSpec() {
        assertEquals(
            "SanDisk Ultra(07815583)",
            Validation.formatAttachedPickerItem("SanDisk Ultra", "0781", "5583"),
        )
    }

    @Test
    fun reverseClientValidationChecksHostPortAnd64HexFingerprint() {
        val valid = Validation.parseReverseClient("192.168.1.10:37425", "A4:F2:" + "00:".repeat(29) + "00")
        assertEquals("192.168.1.10:37425", valid?.first)
        assertEquals(64, valid?.second?.length)

        assertNull(Validation.parseReverseClient("no-port", "a".repeat(64)))
        assertNull(Validation.parseReverseClient("192.168.1.10:37425", "deadbeef"))
        assertNull(Validation.parseReverseClient("192.168.1.10:99999", "a".repeat(64)))
    }
}
