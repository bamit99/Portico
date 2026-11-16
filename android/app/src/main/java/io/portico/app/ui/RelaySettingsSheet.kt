package io.portico.app.ui

import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
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
import io.portico.app.ui.theme.BrandBlueDim
import io.portico.app.ui.theme.BrandGreen
import io.portico.app.ui.theme.CrimsonDanger

@Composable
fun RelaySettingsDialog(
    relayEnabled: Boolean,
    relayStatus: String,
    relayAddress: String,
    currentEndpoint: String,
    onApply: (Boolean, String) -> Unit,
    onToast: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var enabled by remember { mutableStateOf(relayEnabled) }
    var endpoint by remember { mutableStateOf(currentEndpoint) }
    val clipboard = LocalClipboardManager.current

    PorticoSheet(
        title = "NAT Relay Service",
        onDismiss = onDismiss,
        buttons = {
            Button(
                onClick = {
                    if (enabled && endpoint.trim().isEmpty()) {
                        onToast("Enter a relay host:port before enabling")
                    } else {
                        onApply(enabled, endpoint)
                        onDismiss()
                    }
                },
                modifier = Modifier.weight(1f),
            ) { Text("Save") }
            SheetPillButton("Cancel", onClick = onDismiss, modifier = Modifier.weight(1f))
        },
    ) {
        Text(
            "Connect behind NAT without port forwarding. The relay forwards end-to-end TLS 1.3 ciphertext only.",
            style = MaterialTheme.typography.bodySmall,
        )
        Row(
            verticalAlignment = Alignment.CenterVertically,
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Enable relay service", modifier = Modifier.weight(1f))
            androidx.compose.material3.Switch(checked = enabled, onCheckedChange = { enabled = it })
        }
        OutlinedTextField(
            value = endpoint,
            onValueChange = { endpoint = it },
            label = { Text("Relay server (host:port)") },
            placeholder = { Text("e.g. 192.168.1.10:37422") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            StatusDot(
                color = when (relayStatus) {
                    "ready" -> BrandGreen
                    "initializing" -> BrandAmber
                    "error" -> CrimsonDanger
                    else -> BrandBlueDim
                },
            )
            Spacer(Modifier.size(8.dp))
            Text(
                PorticoStateReducer.relayStatusLabel(relayStatus, relayAddress),
                style = MaterialTheme.typography.bodySmall,
                fontFamily = if (relayAddress.isNotBlank()) FontFamily.Monospace else FontFamily.Default,
            )
        }
        if (relayAddress.isNotBlank()) {
            SheetPillButton(
                label = "Copy Relay Address",
                onClick = {
                    clipboard.setText(AnnotatedString(relayAddress))
                    onToast("Copied relay address")
                },
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}
