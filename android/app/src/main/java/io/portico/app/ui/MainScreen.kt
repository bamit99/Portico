package io.portico.app.ui

import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.portico.app.control.DeviceItem
import io.portico.app.ui.theme.AmberWarn
import io.portico.app.ui.theme.TealSignal

private enum class ActiveDialog {
    NONE,
    LOGS,
    RELAY,
    IGNORED,
    REVERSE,
    SECURITY,
    CLIENT_SETUP,
    HELP,
    ABOUT,
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MainScreen(
    viewModel: PorticoViewModel,
    onExitApp: () -> Unit,
) {
    val state by viewModel.uiState.collectAsState()
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current

    var menuExpanded by remember { mutableStateOf(false) }
    var activeDialog by remember { mutableStateOf(ActiveDialog.NONE) }
    var showToken by remember { mutableStateOf(false) }

    LaunchedEffect(state.toastMessage) {
        val msg = state.toastMessage
        if (msg != null) {
            Toast.makeText(context, msg, Toast.LENGTH_SHORT).show()
            viewModel.clearToast()
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Portico", fontWeight = FontWeight.SemiBold)
                        Spacer(Modifier.width(10.dp))
                        Surface(
                            color = if (state.daemon.running) TealSignal.copy(alpha = 0.16f) else AmberWarn.copy(alpha = 0.16f),
                            shape = MaterialTheme.shapes.small,
                        ) {
                            Text(
                                text = if (state.daemon.running) "Sharing" else "Paused",
                                color = if (state.daemon.running) TealSignal else AmberWarn,
                                style = MaterialTheme.typography.labelMedium,
                                modifier = Modifier.padding(horizontal = 8.dp, vertical = 4.dp),
                            )
                        }
                    }
                },
                actions = {
                    IconButton(onClick = { viewModel.rescanUsb() }) {
                        Icon(Icons.Default.Refresh, contentDescription = "Rescan USB")
                    }
                    IconButton(onClick = { viewModel.toggleServerRunning() }) {
                        Icon(
                            imageVector = if (state.daemon.running) Icons.Default.Stop else Icons.Default.PlayArrow,
                            contentDescription = if (state.daemon.running) "Stop server" else "Start server",
                        )
                    }
                    Box {
                        IconButton(onClick = { menuExpanded = true }) {
                            Icon(Icons.Default.MoreVert, contentDescription = "Menu")
                        }
                        DropdownMenu(
                            expanded = menuExpanded,
                            onDismissRequest = { menuExpanded = false },
                        ) {
                            DropdownMenuItem(
                                text = { Text("1. System messages / log") },
                                onClick = { menuExpanded = false; activeDialog = ActiveDialog.LOGS },
                            )
                            DropdownMenuItem(
                                text = {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text("2. Start on boot", modifier = Modifier.weight(1f))
                                        Switch(
                                            checked = state.daemon.startOnBoot,
                                            onCheckedChange = { viewModel.setStartOnBoot(context, it) },
                                        )
                                    }
                                },
                                onClick = { viewModel.setStartOnBoot(context, !state.daemon.startOnBoot) },
                            )
                            DropdownMenuItem(
                                text = {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text("3. Keep network awake", modifier = Modifier.weight(1f))
                                        Switch(
                                            checked = state.daemon.netLock,
                                            onCheckedChange = { viewModel.setNetLock(it) },
                                        )
                                    }
                                },
                                onClick = { viewModel.setNetLock(!state.daemon.netLock) },
                            )
                            DropdownMenuItem(
                                text = {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text("4. Keep device awake", modifier = Modifier.weight(1f))
                                        Switch(
                                            checked = state.daemon.wakeLock,
                                            onCheckedChange = { viewModel.setWakeLock(it) },
                                        )
                                    }
                                },
                                onClick = { viewModel.setWakeLock(!state.daemon.wakeLock) },
                            )
                            DropdownMenuItem(
                                text = { Text("5. NAT relay service…") },
                                onClick = { menuExpanded = false; activeDialog = ActiveDialog.RELAY },
                            )
                            DropdownMenuItem(
                                text = { Text("6. Ignored devices…") },
                                onClick = { menuExpanded = false; activeDialog = ActiveDialog.IGNORED },
                            )
                            DropdownMenuItem(
                                text = { Text("7. Reverse clients…") },
                                onClick = { menuExpanded = false; activeDialog = ActiveDialog.REVERSE },
                            )
                            DropdownMenuItem(
                                text = { Text("8. TLS / Security settings…") },
                                onClick = { menuExpanded = false; activeDialog = ActiveDialog.SECURITY },
                            )
                            DropdownMenuItem(
                                text = { Text("9. Help") },
                                onClick = { menuExpanded = false; activeDialog = ActiveDialog.HELP },
                            )
                            DropdownMenuItem(
                                text = { Text("10. About") },
                                onClick = { menuExpanded = false; activeDialog = ActiveDialog.ABOUT },
                            )
                            DropdownMenuItem(
                                text = { Text("11. Exit") },
                                onClick = {
                                    menuExpanded = false
                                    viewModel.shutdownAndExit(onExitApp)
                                },
                            )
                        }
                    }
                },
            )
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 16.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(modifier = Modifier.padding(16.dp)) {
                        Text(
                            text = if (state.daemon.running) {
                                "USB devices plugged into this phone are shared over your network once you grant USB access."
                            } else {
                                "Sharing is paused. Start the service to share attached USB devices."
                            },
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        Spacer(Modifier.height(8.dp))
                        TextButton(onClick = { activeDialog = ActiveDialog.CLIENT_SETUP }) {
                            Text("Desktop client setup & command →")
                        }
                    }
                }
            }

            item {
                Card(modifier = Modifier.fillMaxWidth()) {
                    Column(modifier = Modifier.padding(16.dp)) {
                        Row(
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.SpaceBetween,
                            modifier = Modifier.fillMaxWidth(),
                        ) {
                            Text(
                                "${state.lanIp}:${state.daemon.port}",
                                fontFamily = FontFamily.Monospace,
                                fontWeight = FontWeight.Medium,
                            )
                            AssistChip(
                                onClick = { activeDialog = ActiveDialog.SECURITY },
                                label = {
                                    Text(
                                        if (state.daemon.plaintextLan) "TLS + LAN Plaintext" else "TLS 1.3 · TOFU",
                                    )
                                },
                            )
                        }

                        if (state.daemon.plaintextLan) {
                            Spacer(Modifier.height(6.dp))
                            Surface(
                                color = MaterialTheme.colorScheme.errorContainer,
                                shape = MaterialTheme.shapes.small,
                                modifier = Modifier.fillMaxWidth(),
                            ) {
                                Text(
                                    "Warning: Plaintext LAN opt-in enabled. USB keystrokes/storage are unencrypted on LAN.",
                                    color = MaterialTheme.colorScheme.onErrorContainer,
                                    style = MaterialTheme.typography.bodySmall,
                                    modifier = Modifier.padding(8.dp),
                                )
                            }
                        }

                        Spacer(Modifier.height(8.dp))
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                "FP: ${Validation.formatShortFingerprint(state.daemon.fingerprintHex)}",
                                fontFamily = FontFamily.Monospace,
                                style = MaterialTheme.typography.bodySmall,
                                modifier = Modifier.weight(1f),
                            )
                            IconButton(onClick = {
                                clipboard.setText(AnnotatedString(state.daemon.fingerprintHex))
                                viewModel.showToast("Copied full SHA-256 fingerprint")
                            }) {
                                Icon(Icons.Default.ContentCopy, contentDescription = "Copy fingerprint")
                            }
                        }

                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(
                                text = "Token: " + if (showToken) state.claimToken else "••••-••••-••••-••••",
                                fontFamily = FontFamily.Monospace,
                                style = MaterialTheme.typography.bodySmall,
                                modifier = Modifier.weight(1f),
                            )
                            IconButton(onClick = { showToken = !showToken }) {
                                Icon(
                                    if (showToken) Icons.Default.VisibilityOff else Icons.Default.Visibility,
                                    contentDescription = "Reveal token",
                                )
                            }
                            IconButton(onClick = {
                                clipboard.setText(AnnotatedString(state.claimToken))
                                viewModel.showToast("Copied claim token")
                            }) {
                                Icon(Icons.Default.ContentCopy, contentDescription = "Copy token")
                            }
                        }
                    }
                }
            }

            item {
                Card(
                    colors = CardDefaults.cardColors(
                        containerColor = MaterialTheme.colorScheme.surfaceVariant,
                    ),
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.padding(14.dp),
                    ) {
                        Column(modifier = Modifier.weight(1f)) {
                            Text("NAT Relay", style = MaterialTheme.typography.labelLarge)
                            Text(
                                PorticoStateReducer.relayStatusLabel(
                                    state.daemon.relayStatus,
                                    state.daemon.relayAddress,
                                ),
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                        TextButton(onClick = { activeDialog = ActiveDialog.RELAY }) {
                            Text("Configure")
                        }
                    }
                }
            }

            item {
                Text(
                    "Attached USB Devices (${state.devices.size})",
                    style = MaterialTheme.typography.titleMedium,
                    fontWeight = FontWeight.Medium,
                )
            }

            if (state.devices.isEmpty()) {
                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Text(
                            "No USB devices attached. Plug in a device via USB-C or an OTG adapter.",
                            style = MaterialTheme.typography.bodyMedium,
                            modifier = Modifier.padding(16.dp),
                        )
                    }
                }
            } else {
                items(state.devices, key = { it.busId }) { dev ->
                    DeviceRow(
                        device = dev,
                        onGrantPermission = { viewModel.requestUsbPermission(dev.busId) },
                        onIgnore = {
                            viewModel.addIgnoredDevice("${dev.vidHex.uppercase()}/${dev.pidHex.uppercase()}")
                        },
                    )
                }
            }
        }
    }

    when (activeDialog) {
        ActiveDialog.LOGS -> SystemLogDialog(
            logs = state.logs,
            onClear = { viewModel.clearLogs() },
            onToast = { viewModel.showToast(it) },
            onDismiss = { activeDialog = ActiveDialog.NONE },
        )
        ActiveDialog.RELAY -> RelaySettingsDialog(
            relayStatus = state.daemon.relayStatus,
            relayAddress = state.daemon.relayAddress,
            currentEndpoint = state.relayEndpointDraft,
            onApply = { en, ep -> viewModel.setRelay(en, ep) },
            onToast = { viewModel.showToast(it) },
            onDismiss = { activeDialog = ActiveDialog.NONE },
        )
        ActiveDialog.IGNORED -> IgnoredDevicesDialog(
            ignoredList = state.ignoredDevices,
            attachedDevices = state.devices,
            onSave = { viewModel.setIgnoredDevices(it) },
            onToast = { viewModel.showToast(it) },
            onDismiss = { activeDialog = ActiveDialog.NONE },
        )
        ActiveDialog.REVERSE -> ReverseClientsDialog(
            entries = state.reverseClients,
            onSave = { viewModel.setReverseClients(it) },
            onToast = { viewModel.showToast(it) },
            onDismiss = { activeDialog = ActiveDialog.NONE },
        )
        ActiveDialog.SECURITY -> SecuritySettingsDialog(
            fingerprintHex = state.daemon.fingerprintHex,
            claimToken = state.claimToken,
            plaintextLan = state.daemon.plaintextLan,
            onTogglePlaintextLan = { viewModel.setPlaintextLan(it) },
            onRegenToken = { viewModel.regenToken() },
            onRotateIdentity = { viewModel.rotateIdentity() },
            onToast = { viewModel.showToast(it) },
            onDismiss = { activeDialog = ActiveDialog.NONE },
        )
        ActiveDialog.CLIENT_SETUP -> DesktopClientSetupDialog(
            lanIp = state.lanIp,
            port = state.daemon.port,
            fingerprintHex = state.daemon.fingerprintHex,
            claimToken = state.claimToken,
            onToast = { viewModel.showToast(it) },
            onDismiss = { activeDialog = ActiveDialog.NONE },
        )
        ActiveDialog.HELP -> HelpDialog(onDismiss = { activeDialog = ActiveDialog.NONE })
        ActiveDialog.ABOUT -> AboutDialog(onDismiss = { activeDialog = ActiveDialog.NONE })
        ActiveDialog.NONE -> {}
    }
}

@Composable
private fun DeviceRow(
    device: DeviceItem,
    onGrantPermission: () -> Unit,
    onIgnore: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.padding(14.dp),
        ) {
            Column(modifier = Modifier.weight(1f)) {
                Text(device.product, fontWeight = FontWeight.Medium)
                Text(
                    "${device.vidHex}/${device.pidHex} · bus ${device.busId} · ${device.state}",
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
            }
            if (device.state == "pending_perm") {
                FilledTonalButton(onClick = onGrantPermission) {
                    Text("Grant USB")
                }
            } else if (device.state != "ignored") {
                OutlinedButton(onClick = onIgnore) {
                    Text("Ignore")
                }
            }
        }
    }
}
