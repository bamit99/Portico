package io.portico.app.ui

import android.content.Context
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import io.portico.app.control.ControlChannelClient
import io.portico.app.control.ControlChannelParser
import io.portico.app.control.ControlEvent
import io.portico.app.control.ControlReply
import io.portico.app.control.DaemonStateSnapshot
import io.portico.app.control.DeviceItem
import io.portico.app.control.LogItem
import io.portico.app.control.ReverseClientItem
import io.portico.app.service.BootPolicy
import io.portico.app.service.PorticoService
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.net.Inet4Address
import java.net.NetworkInterface

data class PorticoUiState(
    val connected: Boolean = false,
    val daemon: DaemonStateSnapshot = DaemonStateSnapshot(),
    val devices: List<DeviceItem> = emptyList(),
    val ignoredDevices: List<String> = emptyList(),
    val reverseClients: List<ReverseClientItem> = emptyList(),
    val logs: List<LogItem> = emptyList(),
    val claimToken: String = "",
    val lanIp: String = "127.0.0.1",
    val relayEndpointDraft: String = "",
    val toastMessage: String? = null,
)

object PorticoStateReducer {
    fun relayStatusLabel(status: String, address: String): String {
        return when (status) {
            "initializing" -> "Please wait, initializing…"
            "ready" -> if (address.isNotBlank()) {
                "Available — put $address in any client to connect!"
            } else {
                "Available — ready for connections!"
            }
            "error" -> "Relay unavailable — retrying…"
            else -> "Currently disabled"
        }
    }

    fun applyEvent(current: PorticoUiState, ev: ControlEvent): PorticoUiState {
        return when (ev.type) {
            "SERVER_STATE" -> {
                val running = ev.fields["running"] == "true"
                val port = ev.fields["port"]?.toIntOrNull() ?: current.daemon.port
                current.copy(daemon = current.daemon.copy(running = running, serverActive = running, port = port))
            }
            "RELAY_STATE" -> {
                val st = ev.fields["status"] ?: current.daemon.relayStatus
                val addr = ev.fields["address"].orEmpty()
                current.copy(daemon = current.daemon.copy(relayStatus = st, relayAddress = addr))
            }
            "LOCK_CHANGED" -> {
                val net = ev.fields["net_lock"] == "true"
                val wake = ev.fields["wake_lock"] == "true"
                current.copy(daemon = current.daemon.copy(netLock = net, wakeLock = wake))
            }
            "DEVICE_ADDED" -> {
                val busId = ev.fields["bus_id"] ?: return current
                val item = DeviceItem(
                    busId = busId,
                    vidHex = ev.fields["vid"] ?: "0000",
                    pidHex = ev.fields["pid"] ?: "0000",
                    product = ev.fields["product"] ?: "USB Device",
                    manufacturer = "",
                    serial = "",
                    state = ev.fields["state"] ?: "shared",
                    peer = "-",
                )
                val updated = current.devices.filterNot { it.busId == busId } + item
                current.copy(devices = updated.sortedBy { it.busId })
            }
            "DEVICE_REMOVED" -> {
                val busId = ev.fields["bus_id"] ?: return current
                current.copy(devices = current.devices.filterNot { it.busId == busId })
            }
            "DEVICE_STATE" -> {
                val busId = ev.fields["bus_id"] ?: return current
                val st = ev.fields["state"] ?: return current
                val peer = ev.fields["peer"] ?: "-"
                current.copy(
                    devices = current.devices.map {
                        if (it.busId == busId) it.copy(state = st, peer = peer) else it
                    },
                )
            }
            "LOG" -> {
                val seq = ev.fields["seq"]?.toLongOrNull() ?: return current
                val entry = LogItem(
                    seq = seq,
                    timestampMs = ev.fields["ts"]?.toLongOrNull() ?: 0L,
                    level = ev.fields["level"] ?: "INFO",
                    target = ev.fields["target"].orEmpty(),
                    message = ev.fields["msg"].orEmpty(),
                )
                val nextLogs = (current.logs + entry).takeLast(500)
                current.copy(logs = nextLogs)
            }
            else -> current
        }
    }
}

class PorticoViewModel : ViewModel() {
    private val _uiState = MutableStateFlow(PorticoUiState(lanIp = detectLanIp()))
    val uiState: StateFlow<PorticoUiState> = _uiState.asStateFlow()

    private var client: ControlChannelClient? = null
    private var serviceRef: PorticoService? = null
    private var eventJob: Job? = null

    fun attachService(service: PorticoService) {
        if (serviceRef === service) return
        serviceRef = service
        val c = service.controlClient
        client = c
        eventJob?.cancel()
        eventJob = viewModelScope.launch {
            c.subscribeEvents().collect { ev ->
                _uiState.update { PorticoStateReducer.applyEvent(it, ev) }
                if (ev.type == "LOCK_CHANGED" || ev.type == "SERVER_STATE") {
                    val st = _uiState.value.daemon
                    serviceRef?.applyLocks(st.netLock, st.wakeLock, st.running)
                }
            }
        }
        refreshAll()
    }

    fun clearToast() {
        _uiState.update { it.copy(toastMessage = null) }
    }

    fun showToast(msg: String) {
        _uiState.update { it.copy(toastMessage = msg) }
    }

    fun refreshAll() {
        val c = client ?: return
        viewModelScope.launch {
            val stateReply = c.sendCommand("GET_STATE")
            val devReply = c.sendCommand("LIST_DEVICES")
            val ignReply = c.sendCommand("GET_IGNORED")
            val revReply = c.sendCommand("GET_REVERSE_CLIENTS")
            val tokReply = c.sendCommand("GET_TOKEN")
            val relayReply = c.sendCommand("GET_RELAY")
            val logsReply = c.sendCommand("GET_LOGS 0")

            _uiState.update { cur ->
                var next = cur.copy(lanIp = detectLanIp())
                if (stateReply is ControlReply.Ok) {
                    val snap = ControlChannelParser.parseState(stateReply.line)
                    next = next.copy(connected = true, daemon = snap)
                    serviceRef?.applyLocks(snap.netLock, snap.wakeLock, snap.running)
                }
                if (devReply is ControlReply.Ok) {
                    next = next.copy(devices = ControlChannelParser.parseDevices(devReply.items))
                }
                if (ignReply is ControlReply.Ok) {
                    next = next.copy(ignoredDevices = ControlChannelParser.parseIgnored(ignReply.items))
                }
                if (revReply is ControlReply.Ok) {
                    next = next.copy(reverseClients = ControlChannelParser.parseReverseClients(revReply.items))
                }
                if (tokReply is ControlReply.Ok) {
                    next = next.copy(claimToken = tokReply.line.removePrefix("OK ").trim())
                }
                if (relayReply is ControlReply.Ok) {
                    val kv = ControlChannelParser.parseKvTokens(
                        relayReply.line.removePrefix("OK ").trim().split(' ').filter { it.isNotEmpty() },
                    )
                    val ep = kv["endpoint"].orEmpty().let { if (it == "-") "" else it }
                    next = next.copy(relayEndpointDraft = ep)
                }
                if (logsReply is ControlReply.Ok) {
                    next = next.copy(logs = ControlChannelParser.parseLogs(logsReply.items))
                }
                next
            }
        }
    }

    fun toggleServerRunning() {
        val running = _uiState.value.daemon.running
        runControl(if (running) "STOP_SERVER" else "START_SERVER")
    }

    fun setStartOnBoot(context: Context, enabled: Boolean) {
        BootPolicy.setStartOnBoot(context, enabled)
        runControl("SET_STATE start_on_boot $enabled")
    }

    fun setNetLock(enabled: Boolean) = runControl("SET_NET_LOCK $enabled")
    fun setWakeLock(enabled: Boolean) = runControl("SET_WAKE_LOCK $enabled")
    fun setPlaintextLan(enabled: Boolean) = runControl("SET_STATE plaintext_lan $enabled")

    fun setRelay(enabled: Boolean, endpoint: String) {
        val c = client ?: return
        viewModelScope.launch {
            val cmd = if (endpoint.trim().isNotEmpty()) {
                "SET_RELAY $enabled ${endpoint.trim()}"
            } else {
                "SET_RELAY $enabled"
            }
            when (val r = c.sendCommand(cmd)) {
                is ControlReply.Ok -> refreshAll()
                is ControlReply.Err -> showToast(r.message.ifBlank { r.code })
            }
        }
    }

    fun setIgnoredDevices(list: List<String>) {
        val arg = if (list.isEmpty()) "-" else list.joinToString(",")
        runControl("SET_IGNORED $arg")
    }

    fun addIgnoredDevice(vidPid: String) {
        val current = _uiState.value.ignoredDevices.toMutableList()
        if (vidPid !in current) {
            current.add(vidPid)
            setIgnoredDevices(current)
        } else {
            showToast("$vidPid is already ignored")
        }
    }

    fun setReverseClients(entries: List<ReverseClientItem>) {
        val arg = if (entries.isEmpty()) {
            "-"
        } else {
            entries.joinToString(",") { "${it.addr}@${it.certSha256}@${it.enabled}" }
        }
        runControl("SET_REVERSE_CLIENTS $arg")
    }

    fun regenToken() = runControl("REGEN_TOKEN")
    fun rotateIdentity() = runControl("ROTATE_IDENTITY")
    fun clearLogs() = runControl("CLEAR_LOGS")

    fun requestUsbPermission(busId: String) {
        serviceRef?.requestUsbPermission(busId)
    }

    fun rescanUsb() {
        serviceRef?.rescanUsb()
        refreshAll()
    }

    fun shutdownAndExit(onDone: () -> Unit) {
        val c = client
        viewModelScope.launch {
            runCatching { c?.sendCommand("SHUTDOWN") }
            onDone()
        }
    }

    private fun runControl(cmd: String) {
        val c = client ?: return
        viewModelScope.launch {
            when (val r = c.sendCommand(cmd)) {
                is ControlReply.Ok -> refreshAll()
                is ControlReply.Err -> showToast(r.message.ifBlank { r.code })
            }
        }
    }

    companion object {
        fun detectLanIp(): String {
            return runCatching {
                NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
                    .filter { it.isUp && !it.isLoopback }
                    .flatMap { it.inetAddresses.toList() }
                    .filterIsInstance<Inet4Address>()
                    .firstOrNull { !it.isLoopbackAddress }
                    ?.hostAddress ?: "127.0.0.1"
            }.getOrDefault("127.0.0.1")
        }
    }
}
