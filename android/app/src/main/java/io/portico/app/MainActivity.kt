package io.portico.app

import android.Manifest
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.ServiceConnection
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import android.os.IBinder
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.core.content.ContextCompat
import io.portico.app.service.PorticoService
import io.portico.app.ui.MainScreen
import io.portico.app.ui.PorticoViewModel
import io.portico.app.ui.theme.PorticoTheme

class MainActivity : ComponentActivity() {
    private val viewModel: PorticoViewModel by viewModels()
    private var bound = false

    private val notifPermissionLauncher = registerForActivityResult(
        ActivityResultContracts.RequestPermission(),
    ) {}

    private val connection = object : ServiceConnection {
        override fun onServiceConnected(name: ComponentName?, service: IBinder?) {
            val binder = service as? PorticoService.LocalBinder ?: return
            viewModel.attachService(binder.getService())
        }

        override fun onServiceDisconnected(name: ComponentName?) {}
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            if (ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS)
                != PackageManager.PERMISSION_GRANTED
            ) {
                notifPermissionLauncher.launch(Manifest.permission.POST_NOTIFICATIONS)
            }
        }

        val svcIntent = Intent(this, PorticoService::class.java).apply {
            action = PorticoService.ACTION_START_AND_SHARE
        }
        ContextCompat.startForegroundService(this, svcIntent)
        bindService(svcIntent, connection, Context.BIND_AUTO_CREATE)
        bound = true

        setContent {
            PorticoTheme {
                MainScreen(
                    viewModel = viewModel,
                    onExitApp = {
                        if (bound) {
                            unbindService(connection)
                            bound = false
                        }
                        stopService(Intent(this, PorticoService::class.java))
                        finishAndRemoveTask()
                    },
                )
            }
        }
    }

    override fun onDestroy() {
        if (bound) {
            unbindService(connection)
            bound = false
        }
        super.onDestroy()
    }
}
