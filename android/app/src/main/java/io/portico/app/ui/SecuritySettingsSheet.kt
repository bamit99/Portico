package io.portico.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
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
import io.portico.app.ui.theme.BrandAmber
import io.portico.app.ui.theme.CrimsonDanger

@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
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

    PorticoSheet(
        title = "TLS, Security & USB",
        onDismiss = onDismiss,
        buttons = {
            Spacer(Modifier.weight(1f))
            Button(onClick = onDismiss) { Text("Done") }
        },
    ) {
        Column(
            modifier = Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                "TLS 1.3 with offline TOFU fingerprint pinning is always active for remote and relay routes.",
                style = MaterialTheme.typography.bodySmall,
            )
            Text(
                "Server Certificate SHA-256",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                fingerprintHex.ifBlank { "Loading…" },
                style = MaterialTheme.typography.bodySmall,
                fontFamily = FontFamily.Monospace,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SheetPillButton(
                    label = "Copy FP",
                    enabled = fingerprintHex.isNotBlank(),
                    onClick = {
                        clipboard.setText(AnnotatedString(fingerprintHex))
                        onToast("Copied server fingerprint")
                    },
                    modifier = Modifier.weight(1f),
                )
                SheetPillButton(
                    label = "Rotate Key",
                    onClick = onRotateIdentity,
                    modifier = Modifier.weight(1f),
                )
            }

            Text(
                "Claim Token",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                claimToken.ifBlank { "Loading…" },
                style = MaterialTheme.typography.bodyMedium,
                fontFamily = FontFamily.Monospace,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SheetPillButton(
                    label = "Copy Token",
                    enabled = claimToken.isNotBlank(),
                    onClick = {
                        clipboard.setText(AnnotatedString(claimToken))
                        onToast("Copied claim token")
                    },
                    modifier = Modifier.weight(1f),
                )
                SheetPillButton(
                    label = "Regenerate",
                    onClick = onRegenToken,
                    modifier = Modifier.weight(1f),
                )
            }

            Text(
                "USB Speed Setting",
                style = MaterialTheme.typography.labelMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            androidx.compose.foundation.layout.FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                listOf("auto", "low", "full", "high", "super").forEach { sp ->
                    FilterChip(
                        selected = usbSpeed == sp,
                        onClick = { onSelectSpeed(sp) },
                        label = { Text(sp) },
                    )
                }
            }

            Row(
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.fillMaxWidth(),
            ) {
                Column(modifier = Modifier.weight(1f)) {
                    Text("Allow plaintext on LAN", style = MaterialTheme.typography.bodyMedium)
                    Text(
                        "Opt-in only for trusted private LANs; public IPs remain blocked.",
                        style = MaterialTheme.typography.bodySmall,
                        color = if (plaintextLan) CrimsonDanger else BrandAmber,
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
    }

    if (confirmPlaintext) {
        PorticoSheet(
            title = "Enable Unencrypted LAN Mode?",
            onDismiss = { confirmPlaintext = false },
            buttons = {
                SheetPillButton(
                    label = "Keep TLS Only",
                    onClick = { confirmPlaintext = false },
                    modifier = Modifier.weight(1f),
                )
                Button(
                    onClick = {
                        confirmPlaintext = false
                        onTogglePlaintextLan(true)
                    },
                    modifier = Modifier.weight(1f),
                ) { Text("Enable LAN Plaintext") }
            },
        ) {
            Text(
                "Warning: USB traffic carries raw keystrokes, storage contents, and serial data. " +
                    "Only allow plaintext connections on an isolated, trusted private LAN.",
                style = MaterialTheme.typography.bodyMedium,
            )
        }
    }
}

