package io.portico.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.portico.app.control.LogItem

@Composable
fun SystemLogDialog(
    logs: List<LogItem>,
    onClear: () -> Unit,
    onToast: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    var filter by remember { mutableStateOf("ALL") }
    val clipboard = LocalClipboardManager.current
    val filtered = remember(logs, filter) {
        if (filter == "ALL") logs else logs.filter { it.level == filter }
    }

    PorticoSheet(
        title = "System Log",
        onDismiss = onDismiss,
        buttons = {
            OutlinedButton(
                onClick = {
                    val text = filtered.joinToString("\n") {
                        "${it.seq} [${it.level}] ${it.target}: ${it.message}"
                    }
                    clipboard.setText(AnnotatedString(text))
                    onToast("Copied ${filtered.size} log lines")
                },
                modifier = Modifier.weight(1f),
            ) {
                Text("Copy All")
            }
            OutlinedButton(
                onClick = onClear,
                modifier = Modifier.weight(1f),
            ) {
                Text("Clear")
            }
            SheetPillButton("Close", onClick = onDismiss, modifier = Modifier.weight(1f))
        },
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf("ALL", "INFO", "WARN", "ERROR").forEach { lvl ->
                FilterChip(
                    selected = filter == lvl,
                    onClick = { filter = lvl },
                    label = { Text(lvl) },
                )
            }
        }
        if (filtered.isEmpty()) {
            Text(
                "No log messages.",
                modifier = Modifier.padding(vertical = 16.dp),
            )
        } else {
            SheetListRegion(maxHeight = 260) {
                items(filtered.asReversed(), key = { it.seq }) { item ->
                    Text(
                        "[${item.level}] ${item.target}: ${item.message}",
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                        modifier = Modifier.padding(vertical = 3.dp),
                    )
                }
            }
        }
        Spacer(Modifier.height(12.dp))
    }
}
