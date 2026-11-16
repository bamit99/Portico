package io.portico.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.portico.app.control.ReverseClientItem

@Composable
fun ReverseClientsDialog(
    entries: List<ReverseClientItem>,
    onSave: (List<ReverseClientItem>) -> Unit,
    onToast: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var addrInput by remember { mutableStateOf("") }
    var fpInput by remember { mutableStateOf("") }
    val selected = remember { mutableStateMapOf<String, Boolean>() }

    PorticoSheet(
        title = "Reverse Clients (mTLS)",
        onDismiss = onDismiss,
        buttons = {
            SheetPillButton("Close", onClick = onDismiss, modifier = Modifier.weight(1f))
        },
    ) {
        Text(
            "Outbound connections require mutual TLS and an allowlisted client certificate SHA-256 fingerprint.",
            style = MaterialTheme.typography.bodySmall,
        )
        OutlinedTextField(
            value = addrInput,
            onValueChange = { addrInput = it },
            label = { Text("Host:Port (e.g. 192.168.1.10:37425)") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        OutlinedTextField(
            value = fpInput,
            onValueChange = { fpInput = it },
            label = { Text("Client Cert SHA-256 (64 hex)") },
            singleLine = true,
            modifier = Modifier.fillMaxWidth(),
        )
        Button(
            onClick = {
                val parsed = Validation.parseReverseClient(addrInput, fpInput)
                if (parsed == null) {
                    onToast("Enter valid host:port and 64-hex SHA-256 fingerprint")
                } else if (entries.any { it.addr == parsed.first }) {
                    onToast("${parsed.first} is already in reverse clients")
                } else {
                    onSave(
                        entries + ReverseClientItem(
                            id = "rev-${entries.size + 1}",
                            addr = parsed.first,
                            certSha256 = parsed.second,
                            enabled = true,
                        ),
                    )
                    addrInput = ""
                    fpInput = ""
                }
            },
            modifier = Modifier.fillMaxWidth(),
        ) {
            Text("Add Reverse Client")
        }
        HorizontalDivider()
        if (entries.isEmpty()) {
            Text(
                "No reverse clients configured.",
                style = MaterialTheme.typography.bodyMedium,
                modifier = Modifier.padding(vertical = 12.dp),
            )
        } else {
            SheetListRegion(maxHeight = 170) {
                items(entries, key = { it.addr }) { item ->
                    Row(
                        verticalAlignment = Alignment.CenterVertically,
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Checkbox(
                            checked = selected[item.addr] == true,
                            onCheckedChange = { selected[item.addr] = it },
                        )
                        Column(modifier = Modifier.weight(1f)) {
                            Text(item.addr, fontFamily = FontFamily.Monospace)
                            Text(
                                Validation.formatShortFingerprint(item.certSha256),
                                style = MaterialTheme.typography.bodySmall,
                                fontFamily = FontFamily.Monospace,
                            )
                        }
                        Switch(
                            checked = item.enabled,
                            onCheckedChange = { en ->
                                onSave(entries.map { if (it.addr == item.addr) it.copy(enabled = en) else it })
                            },
                        )
                    }
                }
            }
            SheetPillButton(
                label = "Remove Selected",
                onClick = {
                    val toRemove = selected.filterValues { it }.keys
                    if (toRemove.isEmpty()) {
                        onToast("Check at least one reverse client to remove")
                    } else {
                        onSave(entries.filterNot { it.addr in toRemove })
                        selected.clear()
                    }
                },
                modifier = Modifier.fillMaxWidth(),
            )
        }
        Spacer(Modifier.size(0.dp))
    }
}
