package io.portico.app

import io.portico.app.control.ControlChannelParser
import io.portico.app.control.DaemonStateSnapshot
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ControlChannelParserTest {

    @Test
    fun parsesGetStateLineAndPercentDecodesRelayAddress() {
        val line =
            "OK running=true server_active=true tls=true plaintext_lan=false port=37420 " +
                "speed=high net_lock=true wake_lock=false boot=true relay_status=ready " +
                "relay_addr=prly%3A%2F%2F127.0.0.1%3A37422%2Faabbccdd fp=deadbeef active_peers=2 tier=free"

        val state: DaemonStateSnapshot = ControlChannelParser.parseState(line)
        assertTrue(state.running)
        assertTrue(state.tlsRequired)
        assertFalse(state.plaintextLan)
        assertEquals(37420, state.port)
        assertEquals("high", state.speed)
        assertTrue(state.netLock)
        assertFalse(state.wakeLock)
        assertTrue(state.startOnBoot)
        assertEquals("ready", state.relayStatus)
        assertEquals("prly://127.0.0.1:37422/aabbccdd", state.relayAddress)
        assertEquals("deadbeef", state.fingerprintHex)
        assertEquals(2, state.activePeers)
    }

    @Test
    fun parsesListDevicesWithPercentEncodedProduct() {
        val lines = listOf(
            "ITEM bus_id=1-2 vid=0781 pid=5583 product=SanDisk%20Ultra%203.0 mfr=SanDisk serial=S1 state=shared peer=-"
        )
        val devices = ControlChannelParser.parseDevices(lines)
        assertEquals(1, devices.size)
        assertEquals("1-2", devices[0].busId)
        assertEquals("0781", devices[0].vidHex)
        assertEquals("5583", devices[0].pidHex)
        assertEquals("SanDisk Ultra 3.0", devices[0].product)
        assertEquals("shared", devices[0].state)
    }

    @Test
    fun parsesLogAndEventLines() {
        val logItems = ControlChannelParser.parseLogs(
            listOf("ITEM seq=4 ts=1700000000000 level=WARN target=tls msg=TLS%20handshake%20failed")
        )
        assertEquals(1, logItems.size)
        assertEquals(4L, logItems[0].seq)
        assertEquals("WARN", logItems[0].level)
        assertEquals("TLS handshake failed", logItems[0].message)

        val ev = ControlChannelParser.parseEvent("EVENT DEVICE_ADDED bus_id=1-3 vid=046D pid=C52B product=Receiver state=shared")
        assertEquals("DEVICE_ADDED", ev?.type)
        assertEquals("1-3", ev?.fields?.get("bus_id"))
    }
}
