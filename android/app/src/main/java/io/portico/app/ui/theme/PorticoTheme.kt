package io.portico.app.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

val SlateNavy = Color(0xFF1E3A5F)
val CobaltAccent = Color(0xFF2563EB)
val TealSignal = Color(0xFF0D9488)
val AmberWarn = Color(0xFFD97706)
val CrimsonDanger = Color(0xFFDC2626)

private val LightColors = lightColorScheme(
    primary = SlateNavy,
    onPrimary = Color.White,
    primaryContainer = Color(0xFFDCE7F5),
    onPrimaryContainer = Color(0xFF0F243D),
    secondary = TealSignal,
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFCCFBF1),
    onSecondaryContainer = Color(0xFF084C46),
    tertiary = CobaltAccent,
    error = CrimsonDanger,
    background = Color(0xFFF6F8FB),
    surface = Color.White,
    surfaceVariant = Color(0xFFE9EEF5),
)

private val DarkColors = darkColorScheme(
    primary = Color(0xFF8BB8F8),
    onPrimary = Color(0xFF0D213B),
    primaryContainer = SlateNavy,
    onPrimaryContainer = Color(0xFFDCE7F5),
    secondary = Color(0xFF5EEAD4),
    onSecondary = Color(0xFF053833),
    secondaryContainer = Color(0xFF0F4C46),
    onSecondaryContainer = Color(0xFFCCFBF1),
    tertiary = CobaltAccent,
    error = Color(0xFFF87171),
    background = Color(0xFF0F172A),
    surface = Color(0xFF1E293B),
    surfaceVariant = Color(0xFF283548),
)

@Composable
fun PorticoTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = if (darkTheme) DarkColors else LightColors,
        content = content,
    )
}
