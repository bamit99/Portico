package io.portico.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Checkbox
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import io.portico.app.control.DeviceItem

@Composable
fun IgnoredDevicesDialog(
    ignoredList: List<String>,
    attachedDevices: List<DeviceItem>,
    onSave: (List<String>) -> Unit,
    onToast: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var input by remember { mutableStateOf("") }
    var showPicker by remember { mutableStateOf(false) }
    val selectedForRemove = remember { mutableStateMapOf<String, Boolean>() }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Ignored Devices") },
        text = {
            Column(modifier = Modifier.fillMaxWidth()) {
                Text(
                    "Devices matching these VID/PID pairs are never auto-shared.",
                    style = MaterialTheme.typography.bodySmall,
                )
                Spacer(Modifier.height(8.dp))
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    OutlinedTextField(
                        value = input,
                        onValueChange = { input = it },
                        label = { Text("VID/PID (e.g. 0123/AE3C)") },
                        singleLine = true,
                        modifier = Modifier.weight(1f),
                    )
                    Button(onClick = {
                        val parsed = Validation.parseVidPid(input)
                        if (parsed == null) {
                            onToast("Enter a valid 4-hex VID/PID pair (e.g. 0123/AE3C)")
                        } else if (parsed in ignoredList) {
                            onToast("$parsed is already in the ignored list")
                        } else {
                            onSave(ignoredList + parsed)
                            input = ""
                        }
                    }) {
                        Text("Add")
                    }
                }
                Spacer(Modifier.height(6.dp))
                OutlinedButton(
                    onClick = { showPicker = true },
                    modifier = Modifier.fillMaxWidth(),
                ) {
                    Text("Select from attached devices")
                }
                Spacer(Modifier.height(8.dp))
                HorizontalDivider()
                if (ignoredList.isEmpty()) {
                    Text(
                        "No ignored devices.",
                        style = MaterialTheme.typography.bodyMedium,
                        modifier = Modifier.padding(vertical = 12.dp),
                    )
                } else {
                    LazyColumn(modifier = Modifier.height(180.dp)) {
                        items(ignoredList) { entry ->
                            Row(
                                verticalAlignment = Alignment.CenterVertically,
                                modifier = Modifier.fillMaxWidth(),
                            ) {
                                Checkbox(
                                    checked = selectedForRemove[entry] == true,
                                    onCheckedChange = { selectedForRemove[entry] = it },
                                )
                                Text(entry, fontFamily = FontFamily.Monospace)
                            }
                        }
                    }
                    OutlinedButton(
                        onClick = {
                            val toRemove = selectedForRemove.filterValues { it }.keys
                            if (toRemove.isEmpty()) {
                                onToast("Check at least one entry to remove")
                            } else {
                                onSave(ignoredList.filterNot { it in toRemove })
                                selectedForRemove.clear()
                            }
                        },
                        modifier = Modifier.fillMaxWidth(),
                    ) {
                        Text("Remove Selected")
                    }
                }
            }
        },
        confirmButton = {
            TextButton(onClick = onDismiss) { Text("Close") }
        },
    )

    if (showPicker) {
        AlertDialog(
            onDismissRequest = { showPicker = false },
            title = { Text("Attached USB Devices") },
            text = {
                if (attachedDevices.isEmpty()) {
                    Text("No USB devices currently attached.")
                } else {
                    LazyColumn(modifier = Modifier.height(200.dp)) {
                        items(attachedDevices) { dev ->
                            val label = Validation.formatAttachedPickerItem(
                                dev.product,
                                dev.vidHex,
                                dev.pidHex,
                            )
                            TextButton(
                                onClick = {
                                    val pair = "${dev.vidHex.uppercase()}/${dev.pidHex.uppercase()}"
                                    if (pair !in ignoredList) {
                                        onSave(ignoredList + pair)
                                    } else {
                                        onToast("$pair is already ignored")
                                    }
                                    showPicker = false
                                },
                                modifier = Modifier.fillMaxWidth(),
                            ) {
                                Text(label, fontFamily = FontFamily.Monospace)
                            }
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { showPicker = false }) { Text("Cancel") }
            },
        )
    }
}
