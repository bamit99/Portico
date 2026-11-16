package io.portico.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp

@OptIn(ExperimentalLayoutApi::class)
@Composable
fun SecuritySettingsDialog(
    fingerprintHex: String,
    claimToken: String,
    plaintextLan: Boolean,
    usbSpeed: String,
    onTogglePlaintextLan: (Boolean) -> Unit,
    onSelectSpeed: (String) -> Unit,
    onRegenToken: () -> Unit,
    onRotateIdentity: () -> Unit,
    onToast: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var confirmPlaintext by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("TLS, Security & USB") },
        text = {
            Column(
                modifier = Modifier
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState()),
            ) {
                Text(
                    "TLS 1.3 with offline TOFU fingerprint pinning is always active for remote and relay routes.",
                    style = MaterialTheme.typography.bodySmall,
                )
                Spacer(Modifier.height(10.dp))
                Text("Server Certificate SHA-256", style = MaterialTheme.typography.labelMedium)
                Text(
                    fingerprintHex.ifBlank { "Loading…" },
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
                Spacer(Modifier.height(6.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedButton(
                        enabled = fingerprintHex.isNotBlank(),
                        onClick = {
                            clipboard.setText(AnnotatedString(fingerprintHex))
                            onToast("Copied server fingerprint")
                        },
                        modifier = Modifier.weight(1f),
                    ) { Text("Copy FP") }
                    OutlinedButton(
                        onClick = onRotateIdentity,
                        modifier = Modifier.weight(1f),
                    ) { Text("Rotate Key") }
                }

                Spacer(Modifier.height(12.dp))
                Text("Claim Token", style = MaterialTheme.typography.labelMedium)
                Text(
                    claimToken.ifBlank { "Loading…" },
                    style = MaterialTheme.typography.bodyMedium,
                    fontFamily = FontFamily.Monospace,
                )
                Spacer(Modifier.height(6.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedButton(
                        enabled = claimToken.isNotBlank(),
                        onClick = {
                            clipboard.setText(AnnotatedString(claimToken))
                            onToast("Copied claim token")
                        },
                        modifier = Modifier.weight(1f),
                    ) { Text("Copy Token") }
                    OutlinedButton(
                        onClick = onRegenToken,
                        modifier = Modifier.weight(1f),
                    ) { Text("Regenerate") }
                }

                Spacer(Modifier.height(12.dp))
                Text("USB Speed Setting", style = MaterialTheme.typography.labelMedium)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    listOf("auto", "low", "full", "high", "super").forEach { sp ->
                        FilterChip(
                            selected = usbSpeed == sp,
                            onClick = { onSelectSpeed(sp) },
                            label = { Text(sp) },
                        )
                    }
                }

                Spacer(Modifier.height(12.dp))
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Column(modifier = Modifier.weight(1f)) {
                        Text("Allow plaintext on LAN", style = MaterialTheme.typography.bodyMedium)
                        Text(
                            "Opt-in only for trusted private LANs; public IPs remain blocked.",
                            style = MaterialTheme.typography.bodySmall,
                            color = if (plaintextLan) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    Switch(
                        checked = plaintextLan,
                        onCheckedChange = { enable ->
                            if (enable) {
                                confirmPlaintext = true
                            } else {
                                onTogglePlaintextLan(false)
                            }
                        },
                    )
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Done") }
        },
    )

    if (confirmPlaintext) {
        AlertDialog(
            onDismissRequest = { confirmPlaintext = false },
            title = { Text("Enable Unencrypted LAN Mode?") },
            text = {
                Text(
                    "Warning: USB traffic carries raw keystrokes, storage contents, and serial data. " +
                        "Only allow plaintext connections on an isolated, trusted private LAN.",
                )
            },
            confirmButton = {
                TextButton(onClick = {
                    confirmPlaintext = false
                    onTogglePlaintextLan(true)
                }) { Text("Enable LAN Plaintext") }
            },
            dismissButton = {
                TextButton(onClick = { confirmPlaintext = false }) { Text("Keep TLS Only") }
            },
        )
    }
}
