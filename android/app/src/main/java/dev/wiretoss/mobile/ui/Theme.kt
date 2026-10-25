package dev.wiretoss.mobile.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp

val WtBackground = Color(0xFF0B1020)
val WtBackgroundDeep = Color(0xFF070A15)
val WtSurface = Color(0xFF151B2E)
val WtText = Color(0xFFE8ECF5)
val WtTextDim = Color(0xFF8A93A8)
val WtTextFaint = Color(0xFF5E6780)
val WtAccent = Color(0xFF5B8CFF)
val WtAccent2 = Color(0xFF8B5CF6)
val WtDanger = Color(0xFFFB7185)
val WtOk = Color(0xFF34D399)

private val WtColors = darkColorScheme(
    primary = WtAccent,
    onPrimary = Color.White,
    primaryContainer = Color(0xFF1B2A55),
    onPrimaryContainer = WtText,
    secondary = WtAccent2,
    onSecondary = Color.White,
    background = WtBackground,
    onBackground = WtText,
    surface = WtSurface,
    onSurface = WtText,
    surfaceVariant = Color(0xFF1C2438),
    onSurfaceVariant = WtTextDim,
    error = WtDanger,
    onError = Color.White,
    outline = Color(0x33FFFFFF),
)

private val WtTypography = Typography(
    bodyLarge = TextStyle(fontSize = 15.sp, lineHeight = 22.sp, fontWeight = FontWeight.Normal),
    bodyMedium = TextStyle(fontSize = 13.sp, lineHeight = 19.sp),
    bodySmall = TextStyle(fontSize = 11.5.sp, lineHeight = 16.sp),
)

@Composable
fun WiretossTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = WtColors, typography = WtTypography, content = content)
}
