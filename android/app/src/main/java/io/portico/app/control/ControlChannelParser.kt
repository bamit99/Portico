package io.portico.app.control

import java.net.URLDecoder

data class DaemonStateSnapshot(
    val running: Boolean = false,
    val serverActive: Boolean = false,
    val tlsRequired: Boolean = true,
    val plaintextLan: Boolean = false,
    val port: Int = 37420,
    val speed: String = "auto",
    val netLock: Boolean = false,
    val wakeLock: Boolean = false,
    val startOnBoot: Boolean = false,
    val relayStatus: String = "disabled",
    val relayAddress: String = "",
    val fingerprintHex: String = "",
    val activePeers: Int = 0,
    val tier: String = "free",
)

data class DeviceItem(
    val busId: String,
    val vidHex: String,
    val pidHex: String,
    val product: String,
    val manufacturer: String,
    val serial: String,
    val state: String,
    val peer: String,
)

data class ReverseClientItem(
    val id: String,
    val addr: String,
    val certSha256: String,
    val enabled: Boolean,
)

data class LogItem(
    val seq: Long,
    val timestampMs: Long,
    val level: String,
    val target: String,
    val message: String,
)

data class ControlEvent(
    val type: String,
    val fields: Map<String, String>,
)

object ControlChannelParser {
    fun pctDecode(raw: String): String {
        return try {
            // Avoid '+' turning into space since percent-encoding uses %20
            URLDecoder.decode(raw.replace("+", "%2B"), "UTF-8")
        } catch (_: Exception) {
            raw
        }
    }

    fun parseKvTokens(tokens: List<String>): Map<String, String> {
        val map = LinkedHashMap<String, String>()
        for (t in tokens) {
            val idx = t.indexOf('=')
            if (idx > 0) {
                val k = t.substring(0, idx)
                val v = pctDecode(t.substring(idx + 1))
                map[k] = v
            }
        }
        return map
    }

    fun parseState(line: String): DaemonStateSnapshot {
        val body = line.removePrefix("OK ").trim()
        val kv = parseKvTokens(body.split(' ').filter { it.isNotEmpty() })
        val relayAddrRaw = kv["relay_addr"].orEmpty()
        return DaemonStateSnapshot(
            running = kv["running"] == "true",
            serverActive = kv["server_active"] == "true",
            tlsRequired = kv["tls"] != "false",
            plaintextLan = kv["plaintext_lan"] == "true",
            port = kv["port"]?.toIntOrNull() ?: 37420,
            speed = kv["speed"] ?: "auto",
            netLock = kv["net_lock"] == "true",
            wakeLock = kv["wake_lock"] == "true",
            startOnBoot = kv["boot"] == "true",
            relayStatus = kv["relay_status"] ?: "disabled",
            relayAddress = if (relayAddrRaw == "-") "" else relayAddrRaw,
            fingerprintHex = kv["fp"].orEmpty(),
            activePeers = kv["active_peers"]?.toIntOrNull() ?: 0,
            tier = kv["tier"] ?: "free",
        )
    }

    fun parseDevices(items: List<String>): List<DeviceItem> {
        return items.mapNotNull { raw ->
            val body = raw.removePrefix("ITEM ").trim()
            if (body.isEmpty()) return@mapNotNull null
            val kv = parseKvTokens(body.split(' ').filter { it.isNotEmpty() })
            DeviceItem(
                busId = kv["bus_id"] ?: return@mapNotNull null,
                vidHex = kv["vid"] ?: "0000",
                pidHex = kv["pid"] ?: "0000",
                product = kv["product"] ?: "USB Device",
                manufacturer = kv["mfr"].orEmpty(),
                serial = kv["serial"].orEmpty(),
                state = kv["state"] ?: "shared",
                peer = kv["peer"] ?: "-",
            )
        }
    }

    fun parseIgnored(items: List<String>): List<String> {
        return items.mapNotNull { raw ->
            val v = raw.removePrefix("ITEM ").trim()
            v.takeIf { it.isNotEmpty() }
        }
    }

    fun parseReverseClients(items: List<String>): List<ReverseClientItem> {
        return items.mapNotNull { raw ->
            val body = raw.removePrefix("ITEM ").trim()
            if (body.isEmpty()) return@mapNotNull null
            val kv = parseKvTokens(body.split(' ').filter { it.isNotEmpty() })
            ReverseClientItem(
                id = kv["id"] ?: "",
                addr = kv["addr"] ?: return@mapNotNull null,
                certSha256 = kv["fp"] ?: return@mapNotNull null,
                enabled = kv["enabled"] != "false",
            )
        }
    }

    fun parseLogs(items: List<String>): List<LogItem> {
        return items.mapNotNull { raw ->
            val body = raw.removePrefix("ITEM ").trim()
            if (body.isEmpty()) return@mapNotNull null
            val kv = parseKvTokens(body.split(' ').filter { it.isNotEmpty() })
            LogItem(
                seq = kv["seq"]?.toLongOrNull() ?: return@mapNotNull null,
                timestampMs = kv["ts"]?.toLongOrNull() ?: 0L,
                level = kv["level"] ?: "INFO",
                target = kv["target"].orEmpty(),
                message = kv["msg"].orEmpty(),
            )
        }
    }

    fun parseEvent(line: String): ControlEvent? {
        if (!line.startsWith("EVENT ")) return null
        val parts = line.removePrefix("EVENT ").trim().split(' ').filter { it.isNotEmpty() }
        if (parts.isEmpty()) return null
        val type = parts[0]
        val fields = parseKvTokens(parts.drop(1))
        return ControlEvent(type = type, fields = fields)
    }
}
