package io.portico.app.ui

import android.widget.Toast
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ContentCopy
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Visibility
import androidx.compose.material.icons.filled.VisibilityOff
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
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import io.portico.app.R
import io.portico.app.control.DeviceItem
import io.portico.app.ui.theme.BrandAmber
import io.portico.app.ui.theme.BrandBlueDim
import io.portico.app.ui.theme.BrandGreen
import io.portico.app.ui.theme.CrimsonDanger
import io.portico.app.ui.theme.HeaderGradientBrush

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

@OptIn(ExperimentalMaterial3Api::class, ExperimentalFoundationApi::class)
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
            // Hero header row on the brand gradient surface.
            TopAppBar(
                colors = androidx.compose.material3.TopAppBarDefaults.topAppBarColors(
                    containerColor = androidx.compose.ui.graphics.Color.Transparent,
                ),
                title = {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Icon(
                            painter = painterResource(R.drawable.ic_portico_usb),
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(26.dp),
                        )
                        Spacer(Modifier.width(10.dp))
                        Text("Portico", fontWeight = FontWeight.SemiBold)
                        Spacer(Modifier.width(10.dp))
                        StatusPill(
                            label = if (state.daemon.running) "Sharing" else "Paused",
                            dotColor = if (state.daemon.running) BrandGreen else BrandBlueDim,
                            labelColor = if (state.daemon.running) BrandGreen else BrandBlueDim,
                        )
                    }
                },
                actions = {
                    CircularGhostIconButton(
                        icon = Icons.Default.Refresh,
                        contentDescription = "Rescan USB",
                        onClick = { viewModel.rescanUsb() },
                    )
                    Spacer(Modifier.width(4.dp))
                    CircularGhostIconButton(
                        icon = if (state.daemon.running) Icons.Default.Stop else Icons.Default.PlayArrow,
                        contentDescription = if (state.daemon.running) "Stop server" else "Start server",
                        onClick = { viewModel.toggleServerRunning() },
                    )
                    Spacer(Modifier.width(4.dp))
                    Box {
                        CircularGhostIconButton(
                            icon = Icons.Default.MoreVert,
                            contentDescription = "Menu",
                            onClick = { menuExpanded = true },
                        )
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
                modifier = Modifier.background(HeaderGradientBrush),
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
                PorticoCard(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        text = if (state.daemon.running) {
                            "USB devices plugged into this phone are shared over your network once you grant USB access."
                        } else {
                            "Sharing is paused. Start the service to share attached USB devices."
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurface,
                    )
                    Spacer(Modifier.height(8.dp))
                    TextButton(onClick = { activeDialog = ActiveDialog.CLIENT_SETUP }) {
                        Text("Desktop client setup & command →")
                    }
                }
            }

            item {
                PorticoCard(modifier = Modifier.fillMaxWidth()) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        MonoValuePill(
                            text = "${state.lanIp}:${state.daemon.port}",
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        PillSurface(onClick = { activeDialog = ActiveDialog.SECURITY }) {
                            StatusDot(
                                color = if (state.daemon.plaintextLan) BrandAmber else BrandGreen,
                            )
                            Text(
                                text = if (state.daemon.plaintextLan) "TLS + LAN Plaintext" else "TLS 1.3 · TOFU",
                                style = MaterialTheme.typography.labelLarge,
                                color = if (state.daemon.plaintextLan) BrandAmber else BrandGreen,
                            )
                        }
                    }

                    AnimatedVisibility(visible = state.daemon.plaintextLan, enter = EnterFadeSlide, exit = ExitFadeSlide) {
                        Surface(
                            color = CrimsonDanger.copy(alpha = 0.12f),
                            shape = MaterialTheme.shapes.medium,
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 8.dp),
                        ) {
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier.padding(10.dp),
                            ) {
                                StatusDot(color = CrimsonDanger)
                                Spacer(Modifier.width(8.dp))
                                Text(
                                    "Warning: Plaintext LAN opt-in enabled. USB keystrokes/storage are unencrypted on LAN.",
                                    color = MaterialTheme.colorScheme.onErrorContainer,
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                        }
                    }

                    Spacer(Modifier.height(10.dp))
                    MonoValueRow(
                        label = "FP",
                        value = Validation.formatShortFingerprint(state.daemon.fingerprintHex),
                    ) {
                        CompactCopyButton(
                            enabled = state.daemon.fingerprintHex.isNotBlank(),
                            contentDescription = "Copy fingerprint",
                            onClick = {
                                clipboard.setText(AnnotatedString(state.daemon.fingerprintHex))
                                viewModel.showToast("Copied full SHA-256 fingerprint")
                            },
                        )
                    }

                    Spacer(Modifier.height(6.dp))
                    MonoValueRow(
                        label = "Token",
                        value = if (showToken) state.claimToken else "••••-••••-••••-••••",
                    ) {
                        IconButton(onClick = { showToken = !showToken }) {
                            Icon(
                                if (showToken) Icons.Default.VisibilityOff else Icons.Default.Visibility,
                                contentDescription = "Reveal token",
                                modifier = Modifier.size(18.dp),
                            )
                        }
                        CompactCopyButton(
                            enabled = state.claimToken.isNotBlank(),
                            contentDescription = "Copy token",
                            onClick = {
                                clipboard.setText(AnnotatedString(state.claimToken))
                                viewModel.showToast("Copied claim token")
                            },
                        )
                    }
                }
            }

            item {
                PorticoCard(
                    containerColor = MaterialTheme.colorScheme.surfaceVariant,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Icon(
                            painter = painterResource(R.drawable.ic_portico_usb),
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.primary,
                            modifier = Modifier.size(22.dp),
                        )
                        Spacer(Modifier.width(12.dp))
                        Column(modifier = Modifier.weight(1f)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                StatusDot(color = relayStatusColor(state.daemon.relayStatus))
                                Spacer(Modifier.width(8.dp))
                                Text("NAT Relay", style = MaterialTheme.typography.labelLarge)
                            }
                            Text(
                                PorticoStateReducer.relayStatusLabel(
                                    state.daemon.relayStatus,
                                    state.daemon.relayAddress,
                                ),
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        PillSurface(onClick = { activeDialog = ActiveDialog.RELAY }) {
                            Text(
                                text = "Configure",
                                style = MaterialTheme.typography.labelLarge,
                                color = MaterialTheme.colorScheme.primary,
                            )
                        }
                    }
                }
            }

            item {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        "Attached USB Devices",
                        style = MaterialTheme.typography.titleMedium,
                        fontWeight = FontWeight.Medium,
                    )
                    Spacer(Modifier.width(8.dp))
                    PillSurface(containerColor = MaterialTheme.colorScheme.primary.copy(alpha = 0.12f)) {
                        Text(
                            text = state.devices.size.toString(),
                            style = MaterialTheme.typography.labelLarge,
                            color = MaterialTheme.colorScheme.primary,
                        )
                    }
                }
            }

            if (state.devices.isEmpty()) {
                item {
                    AnimatedVisibility(visible = true, enter = EnterFadeSlideSlow) {
                        PorticoCard(modifier = Modifier.fillMaxWidth()) {
                            Text(
                                "No USB devices attached. Plug in a device via USB-C or an OTG adapter.",
                                style = MaterialTheme.typography.bodyMedium,
                            )
                        }
                    }
                }
            } else {
                items(state.devices, key = { it.busId }) { dev ->
                    AnimatedVisibility(
                        visible = true,
                        enter = EnterFadeSlideSlow,
                        exit = ExitFadeSlide,
                        modifier = Modifier.animateItem(),
                    ) {
                        DeviceRow(
                            device = dev,
                            onGrantPermission = { viewModel.requestUsbPermission(dev.busId) },
                            onIgnore = {
                                viewModel.addIgnoredDevice("${dev.vidHex.uppercase()}/${dev.pidHex.uppercase()}")
                            },
                            onRelease = { viewModel.releaseDevice(dev.busId) },
                        )
                    }
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
            relayEnabled = state.relayEnabled,
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
            usbSpeed = state.daemon.speed,
            onTogglePlaintextLan = { viewModel.setPlaintextLan(it) },
            onSelectSpeed = { viewModel.setUsbSpeed(it) },
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
    onRelease: () -> Unit,
) {
    PorticoCard(modifier = Modifier.fillMaxWidth()) {
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Box(
                modifier = Modifier
                    .size(40.dp)
                    .background(
                        MaterialTheme.colorScheme.primary.copy(alpha = 0.12f),
                        MaterialTheme.shapes.small,
                    ),
                contentAlignment = Alignment.Center,
            ) {
                Icon(
                    painter = painterResource(R.drawable.ic_portico_usb),
                    contentDescription = null,
                    tint = MaterialTheme.colorScheme.primary,
                    modifier = Modifier.size(22.dp),
                )
            }
            Spacer(Modifier.width(12.dp))
            Column(modifier = Modifier.weight(1f)) {
                Text(
                    device.product,
                    fontWeight = FontWeight.Medium,
                    style = MaterialTheme.typography.titleMedium,
                    color = MaterialTheme.colorScheme.onSurface,
                )
                Row(verticalAlignment = Alignment.CenterVertically) {
                    StatusDot(color = deviceStatusColor(device.state))
                    Spacer(Modifier.width(8.dp))
                    Text(
                        text = when (device.state) {
                            "shared" -> "shared"
                            "in_use" -> if (device.peer != "-") "in use by ${device.peer}" else "in use"
                            "ignored" -> "ignored"
                            else -> device.state
                        },
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                Text(
                    "${device.vidHex}/${device.pidHex} · bus ${device.busId}",
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            when (device.state) {
                "pending_perm" -> FilledTonalButton(
                    shape = RoundedCornerShape(percent = 50),
                    onClick = onGrantPermission,
                ) {
                    Text("Grant USB")
                }
                "in_use" -> OutlinedButton(
                    shape = RoundedCornerShape(percent = 50),
                    onClick = onRelease,
                ) {
                    Text("Release")
                }
                "ignored" -> {}
                else -> OutlinedButton(
                    shape = RoundedCornerShape(percent = 50),
                    onClick = onIgnore,
                ) {
                    Text("Ignore")
                }
            }
        }
    }
}

@Composable
private fun StatusPill(
    label: String,
    dotColor: androidx.compose.ui.graphics.Color,
    labelColor: androidx.compose.ui.graphics.Color,
) {
    PillSurface(
        containerColor = labelColor.copy(alpha = 0.14f),
    ) {
        StatusDot(color = dotColor)
        Text(
            text = label,
            style = MaterialTheme.typography.labelLarge,
            color = labelColor,
        )
    }
}

private fun relayStatusColor(status: String): androidx.compose.ui.graphics.Color = when (status) {
    "ready" -> BrandGreen
    "initializing" -> BrandAmber
    "error" -> CrimsonDanger
    else -> BrandBlueDim
}

private fun deviceStatusColor(state: String): androidx.compose.ui.graphics.Color = when (state) {
    "shared" -> BrandGreen
    "in_use" -> BrandAmber
    "ignored" -> BrandBlueDim
    else -> BrandAmber
}
