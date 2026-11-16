package io.portico.app.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import io.portico.app.BuildConfig

@Composable
fun DesktopClientSetupDialog(
    lanIp: String,
    port: Int,
    fingerprintHex: String,
    claimToken: String,
    onToast: (String) -> Unit,
    onDismiss: () -> Unit,
) {
    val clipboard = LocalClipboardManager.current
    val cmd = "portico list $lanIp:$port --trust $fingerprintHex --token $claimToken"

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Desktop Client Setup") },
        text = {
            Column(modifier = Modifier.fillMaxWidth()) {
                Text("Run on your desktop Linux workstation:", style = MaterialTheme.typography.bodySmall)
                Spacer(Modifier.height(8.dp))
                Text(
                    cmd,
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                )
                Spacer(Modifier.height(10.dp))
                OutlinedButton(
                    onClick = {
                        clipboard.setText(AnnotatedString(cmd))
                        onToast("Copied command")
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("Copy Command") }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
fun HelpDialog(onDismiss: () -> Unit) {
    val context = LocalContext.current
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Portico Help") },
        text = {
            Column {
                Text(
                    "1. Start the Portico service and plug in a USB device.\n" +
                        "2. Grant Android USB permission when prompted.\n" +
                        "3. On desktop, run `portico list <phone-ip>:37420 --token <token>` and `portico attach <phone-ip>:37420 <bus_id>`.",
                    style = MaterialTheme.typography.bodySmall,
                )
                Spacer(Modifier.height(10.dp))
                OutlinedButton(
                    onClick = {
                        runCatching {
                            context.startActivity(
                                Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/bamit99/Portico")),
                            )
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                ) { Text("Open Project Site") }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
}

@Composable
fun AboutDialog(onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("About Portico") },
        text = {
            Text(
                "Portico v${BuildConfig.VERSION_NAME}\n\n" +
                    "Clean-room root-free USB-over-network server with TLS 1.3, TOFU certificate pinning, " +
                    "mutual-TLS reverse clients, and ciphertext-only NAT relay.\n\n" +
                    "Open-source core: jiegec/usbip (MIT), rusb (MIT), rustls (Apache-2.0/MIT).\n\n" +
                    "Project: https://github.com/bamit99/Portico",
                style = MaterialTheme.typography.bodySmall,
            )
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
}
