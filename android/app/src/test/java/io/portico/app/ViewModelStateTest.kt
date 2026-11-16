package io.portico.app

import io.portico.app.control.ControlEvent
import io.portico.app.control.DaemonStateSnapshot
import io.portico.app.control.DeviceItem
import io.portico.app.ui.PorticoStateReducer
import io.portico.app.ui.PorticoUiState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ViewModelStateTest {

    @Test
    fun relayStatusLabelsMatchFeatureSpec() {
        assertEquals(
            "Currently disabled",
            PorticoStateReducer.relayStatusLabel("disabled", ""),
        )
        assertEquals(
            "Please wait, initializing…",
            PorticoStateReducer.relayStatusLabel("initializing", ""),
        )
        assertEquals(
            "Available — put prly://r:37422/ab in any client to connect!",
            PorticoStateReducer.relayStatusLabel("ready", "prly://r:37422/ab"),
        )
    }

    @Test
    fun reducerAppliesDeviceAddedAndRemovedEvents() {
        val initial = PorticoUiState(
            daemon = DaemonStateSnapshot(running = true),
            devices = listOf(
                DeviceItem("1-1", "0781", "5583", "Flash", "SanDisk", "S1", "shared", "-"),
            ),
        )
        val added = PorticoStateReducer.applyEvent(
            initial,
            ControlEvent(
                "DEVICE_ADDED",
                mapOf("bus_id" to "1-2", "vid" to "046D", "pid" to "C52B", "product" to "Mouse", "state" to "shared"),
            ),
        )
        assertEquals(2, added.devices.size)
        assertTrue(added.devices.any { it.busId == "1-2" && it.product == "Mouse" })

        val removed = PorticoStateReducer.applyEvent(
            added,
            ControlEvent("DEVICE_REMOVED", mapOf("bus_id" to "1-1")),
        )
        assertEquals(1, removed.devices.size)
        assertFalse(removed.devices.any { it.busId == "1-1" })

        val logged1 = PorticoStateReducer.applyEvent(
            removed,
            ControlEvent("LOG", mapOf("seq" to "10", "ts" to "1", "level" to "INFO", "target" to "t", "msg" to "hi")),
        )
        val loggedDup = PorticoStateReducer.applyEvent(
            logged1,
            ControlEvent("LOG", mapOf("seq" to "10", "ts" to "1", "level" to "INFO", "target" to "t", "msg" to "hi")),
        )
        assertEquals(1, loggedDup.logs.size)
    }
}
