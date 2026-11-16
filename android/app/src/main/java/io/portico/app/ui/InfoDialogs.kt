package io.portico.app.ui

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
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

    PorticoSheet(
        title = "Desktop Client Setup",
        onDismiss = onDismiss,
        buttons = {
            Button(
                onClick = onDismiss,
                modifier = Modifier.weight(1f),
            ) { Text("Close") }
        },
    ) {
        Text("Run on your desktop Linux workstation:", style = MaterialTheme.typography.bodySmall)
        MonoValuePill(
            text = cmd,
            modifier = Modifier.fillMaxWidth(),
        )
        SheetPillButton(
            label = "Copy Command",
            onClick = {
                clipboard.setText(AnnotatedString(cmd))
                onToast("Copied command")
            },
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.weight(1f))
    }
}

@Composable
fun HelpDialog(onDismiss: () -> Unit) {
    val context = LocalContext.current
    PorticoSheet(
        title = "Portico Help",
        onDismiss = onDismiss,
        buttons = {
            Button(
                onClick = onDismiss,
                modifier = Modifier.weight(1f),
            ) { Text("Close") }
        },
    ) {
        Text(
            "1. Start the Portico service and plug in a USB device.\n" +
                "2. Grant Android USB permission when prompted.\n" +
                "3. On desktop, run `portico list <phone-ip>:37420 --token <token>` and `portico attach <phone-ip>:37420 <bus_id>`.",
            style = MaterialTheme.typography.bodySmall,
        )
        SheetPillButton(
            label = "Open Project Site",
            onClick = {
                runCatching {
                    context.startActivity(
                        Intent(Intent.ACTION_VIEW, Uri.parse("https://github.com/bamit99/Portico")),
                    )
                }
            },
            modifier = Modifier.fillMaxWidth(),
        )
        Spacer(Modifier.weight(1f))
    }
}

@Composable
fun AboutDialog(onDismiss: () -> Unit) {
    PorticoSheet(
        title = "About Portico",
        onDismiss = onDismiss,
        buttons = {
            Button(
                onClick = onDismiss,
                modifier = Modifier.weight(1f),
            ) { Text("Close") }
        },
    ) {
        Text(
            "Portico v${BuildConfig.VERSION_NAME}\n\n" +
                "Clean-room root-free USB-over-network server with TLS 1.3, TOFU certificate pinning, " +
                "mutual-TLS reverse clients, and ciphertext-only NAT relay.\n\n" +
                "Open-source core: jiegec/usbip (MIT), rusb (MIT), rustls (Apache-2.0/MIT).\n\n" +
                "Project: https://github.com/bamit99/Portico",
            style = MaterialTheme.typography.bodySmall,
        )
        Spacer(Modifier.weight(1f))
    }
}
