package io.portico.app.control

import android.net.LocalSocket
import android.net.LocalSocketAddress
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.isActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import java.io.BufferedReader
import java.io.BufferedWriter
import java.io.InputStreamReader
import java.io.OutputStreamWriter

sealed class ControlReply {
    data class Ok(val line: String, val items: List<String> = emptyList()) : ControlReply()
    data class Err(val code: String, val message: String) : ControlReply()
}

class ControlChannelClient(private val socketPath: String) {
    private val cmdMutex = Mutex()

    suspend fun sendCommand(command: String): ControlReply = withContext(Dispatchers.IO) {
        cmdMutex.withLock {
            try {
                val socket = LocalSocket()
                socket.connect(
                    LocalSocketAddress(socketPath, LocalSocketAddress.Namespace.FILESYSTEM),
                )
                socket.use { sock ->
                    val writer = BufferedWriter(OutputStreamWriter(sock.outputStream, Charsets.UTF_8))
                    val reader = BufferedReader(InputStreamReader(sock.inputStream, Charsets.UTF_8))

                    writer.write(command)
                    writer.write("\n")
                    writer.flush()

                    val first = reader.readLine()
                        ?: return@withLock ControlReply.Err("eof", "control socket closed")
                    if (first.startsWith("ERR ")) {
                        val rest = first.removePrefix("ERR ").trim()
                        val sp = rest.indexOf(' ')
                        val code = if (sp > 0) rest.substring(0, sp) else rest
                        val msg = if (sp > 0) {
                            ControlChannelParser.pctDecode(rest.substring(sp + 1))
                        } else {
                            ""
                        }
                        return@withLock ControlReply.Err(code, msg)
                    }

                    if (first.startsWith("OK ")) {
                        val count = first.removePrefix("OK ").trim().toIntOrNull()
                        if (count != null) {
                            val items = ArrayList<String>(count)
                            while (true) {
                                val next = reader.readLine() ?: break
                                if (next == "END") break
                                items.add(next)
                            }
                            return@withLock ControlReply.Ok(first, items)
                        }
                    }
                    ControlReply.Ok(first)
                }
            } catch (e: Exception) {
                ControlReply.Err("io_error", e.message ?: "control socket error")
            }
        }
    }

    fun subscribeEvents(): Flow<ControlEvent> = flow {
        while (currentCoroutineContext().isActive) {
            var activeSock: LocalSocket? = null
            try {
                val socket = LocalSocket()
                activeSock = socket
                socket.connect(
                    LocalSocketAddress(socketPath, LocalSocketAddress.Namespace.FILESYSTEM),
                )
                socket.use { sock ->
                    val writer = BufferedWriter(OutputStreamWriter(sock.outputStream, Charsets.UTF_8))
                    val reader = BufferedReader(InputStreamReader(sock.inputStream, Charsets.UTF_8))
                    writer.write("SUBSCRIBE\n")
                    writer.flush()
                    val ack = reader.readLine()
                    if (ack == "OK") {
                        emit(ControlEvent("RECONNECTED", emptyMap()))
                        while (currentCoroutineContext().isActive) {
                            val line = reader.readLine() ?: break
                            ControlChannelParser.parseEvent(line)?.let { emit(it) }
                        }
                    }
                }
            } catch (_: Exception) {
                // Retry while service starts/restarts
            } finally {
                runCatching { activeSock?.close() }
            }
            delay(500)
        }
    }.flowOn(Dispatchers.IO)
}
