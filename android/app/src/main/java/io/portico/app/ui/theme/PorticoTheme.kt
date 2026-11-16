package io.portico.app.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Shapes
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.unit.dp

// Brand palette — matches the launcher icon (res/drawable/ic_launcher_*.xml).
val BrandBackground = Color(0xFF0A1526)
val BrandHeaderTop = Color(0xFF1A2B47)
val BrandSurface = Color(0xFF121F35)
val BrandSurfaceElevated = Color(0xFF16263F)
val BrandHairline = Color(0xFF2A4160)
val BrandBlue = Color(0xFF8FB9F8)
val BrandBlueSoft = Color(0xFFC7DAF6)
val BrandBlueDim = Color(0xFF8FA3C2)
val BrandGreen = Color(0xFF6ED48B)
val BrandAmber = Color(0xFFF5B04B)
val BrandRed = Color(0xFFF27E6D)

/** Top-of-screen header zone gradient (top -> backdrop), matching the launcher artwork. */
val HeaderGradientBrush = Brush.verticalGradient(listOf(BrandHeaderTop, BrandBackground))

// Kept for legacy references; brand-aligned values.
val TealSignal = BrandGreen
val AmberWarn = BrandAmber
val CrimsonDanger = BrandRed
val SlateNavy = Color(0xFF1E3A5F)
val CobaltAccent = BrandBlue

private val LightColors = lightColorScheme(
    primary = Color(0xFF2E537F),
    onPrimary = Color.White,
    primaryContainer = Color(0xFFDCE7F5),
    onPrimaryContainer = Color(0xFF0F243D),
    secondary = Color(0xFF159B5D),
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFCCF2D8),
    onSecondaryContainer = Color(0xFF084C46),
    tertiary = CobaltAccent,
    error = Color(0xFFC94A39),
    background = Color(0xFFF6F8FB),
    surface = Color(0xFFFFFFFF),
    surfaceVariant = Color(0xFFE9EEF5),
    onSurfaceVariant = Color(0xFF51617A),
    outline = Color(0xFFA9BCD8),
)

private val DarkColors = darkColorScheme(
    primary = BrandBlue,
    onPrimary = Color(0xFF0A1526),
    primaryContainer = BrandSurfaceElevated,
    onPrimaryContainer = BrandBlueSoft,
    secondary = BrandGreen,
    onSecondary = Color(0xFF0A1526),
    secondaryContainer = Color(0xFF143322),
    onSecondaryContainer = BrandGreen,
    tertiary = BrandAmber,
    onTertiary = Color(0xFF2A1A05),
    tertiaryContainer = Color(0xFF3A2B12),
    onTertiaryContainer = BrandAmber,
    error = BrandRed,
    onError = Color(0xFF2A100C),
    errorContainer = Color(0xFF3A1D17),
    onErrorContainer = BrandRed,
    background = BrandBackground,
    onBackground = BrandBlueSoft,
    surface = BrandSurface,
    onSurface = BrandBlueSoft,
    surfaceVariant = BrandSurfaceElevated,
    onSurfaceVariant = BrandBlueDim,
    surfaceContainer = BrandSurface,
    surfaceContainerHigh = BrandSurfaceElevated,
    surfaceContainerHighest = BrandSurfaceElevated,
    outline = BrandHairline,
    outlineVariant = BrandHairline,
)

private val PorticoShapes = Shapes(
    extraSmall = RoundedCornerShape(8.dp),
    small = RoundedCornerShape(12.dp),
    medium = RoundedCornerShape(18.dp),
    large = RoundedCornerShape(24.dp),
    extraLarge = RoundedCornerShape(30.dp),
)

@Composable
fun PorticoTheme(
    darkTheme: Boolean = isSystemInDarkTheme(),
    content: @Composable () -> Unit,
) {
    MaterialTheme(
        colorScheme = if (darkTheme) DarkColors else LightColors,
        shapes = PorticoShapes,
        content = content,
    )
}
