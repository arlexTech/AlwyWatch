// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import be.alwyapps.watch.R

// Near-black with a hint of blue; surfaces are translucent white so they pick up the ambient
// artwork glow behind them.
val Background = Color(0xFF09090C)
val SurfaceColor = Color(0x14FFFFFF)
val SurfaceHigh = Color(0x24FFFFFF)
val Outline = Color(0x1FFFFFFF)
val OnSurface = Color(0xFFF4F4F6)
val Muted = Color(0x99F4F4F6)
val Subtle = Color(0x66F4F4F6)
val Accent = Color(0xFFFFB224)
val Danger = Color(0xFFFF5A5F)
val Success = Color(0xFF3DDC97)

/** Sizes that differ between a 10-foot TV UI and a handheld one. */
data class Dimens(
    val isTv: Boolean,
    val gutter: Dp,
    val posterWidth: Dp,
    val landscapeWidth: Dp,
    val railSpacing: Dp,
    val focusScale: Float,
    val radius: Dp,
)

private val PhoneDimens =
    Dimens(isTv = false, gutter = 20.dp, posterWidth = 118.dp, landscapeWidth = 248.dp, railSpacing = 12.dp, focusScale = 1f, radius = 14.dp)
private val TvDimens =
    Dimens(isTv = true, gutter = 56.dp, posterWidth = 150.dp, landscapeWidth = 300.dp, railSpacing = 20.dp, focusScale = 1.07f, radius = 16.dp)

val LocalDimens = staticCompositionLocalOf { PhoneDimens }

/** Manrope (variable font, bundled: ~160 KB, OFL). */
private fun manrope(weight: Int) = Font(R.font.manrope, FontWeight(weight), variationSettings = FontVariation.Settings(FontVariation.weight(weight)))

val Manrope = FontFamily(manrope(400), manrope(500), manrope(600), manrope(700), manrope(800))

private fun style(
    size: TextUnit,
    weight: Int,
    tracking: TextUnit = 0.sp,
    lineHeight: TextUnit = TextUnit.Unspecified,
) = TextStyle(fontFamily = Manrope, fontSize = size, fontWeight = FontWeight(weight), letterSpacing = tracking, lineHeight = lineHeight)

private fun typography(tv: Boolean): Typography {
    val k = if (tv) 1.25f else 1f
    return Typography(
        displayLarge = style((44 * k).sp, 800, (-0.02).em),
        displayMedium = style((36 * k).sp, 800, (-0.02).em),
        displaySmall = style((30 * k).sp, 800, (-0.02).em),
        headlineMedium = style((26 * k).sp, 700, (-0.01).em),
        titleLarge = style((22 * k).sp, 700, (-0.01).em),
        titleMedium = style((17 * k).sp, 700),
        titleSmall = style((15 * k).sp, 600),
        bodyLarge = style((16 * k).sp, 500, lineHeight = (24 * k).sp),
        bodyMedium = style((14 * k).sp, 500, lineHeight = (20 * k).sp),
        bodySmall = style((12.5f * k).sp, 500),
        labelLarge = style((15 * k).sp, 700),
        labelMedium = style((13 * k).sp, 700, 0.02.em),
        labelSmall = style((11 * k).sp, 800, 0.06.em),
    )
}

@Composable
fun AlwyTheme(
    isTv: Boolean,
    content: @Composable () -> Unit,
) {
    val colors =
        darkColorScheme(
            primary = Color.White,
            onPrimary = Color.Black,
            secondary = Accent,
            background = Background,
            onBackground = OnSurface,
            surface = Color(0xFF16161B),
            onSurface = OnSurface,
            surfaceVariant = Color(0xFF202027),
            onSurfaceVariant = Muted,
            outline = Outline,
            error = Danger,
        )
    MaterialTheme(colorScheme = colors, typography = typography(isTv)) {
        CompositionLocalProvider(
            LocalDimens provides if (isTv) TvDimens else PhoneDimens,
            LocalContentColor provides OnSurface,
            content = content,
        )
    }
}
