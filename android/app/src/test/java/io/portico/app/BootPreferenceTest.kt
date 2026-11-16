package io.portico.app

import io.portico.app.service.BootPolicy
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class BootPreferenceTest {

    @Test
    fun bootReceiverStartsServiceOnlyOnBootCompletedWhenEnabled() {
        assertTrue(BootPolicy.shouldStartOnAction("android.intent.action.BOOT_COMPLETED", bootEnabled = true))
        assertFalse(BootPolicy.shouldStartOnAction("android.intent.action.LOCKED_BOOT_COMPLETED", bootEnabled = true))
        assertFalse(BootPolicy.shouldStartOnAction("android.intent.action.BOOT_COMPLETED", bootEnabled = false))
        assertFalse(BootPolicy.shouldStartOnAction("android.intent.action.MY_PACKAGE_REPLACED", bootEnabled = true))
        assertFalse(BootPolicy.shouldStartOnAction(null, bootEnabled = true))
    }
}
