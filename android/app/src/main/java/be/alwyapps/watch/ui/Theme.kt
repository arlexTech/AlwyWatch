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
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

val Background = Color(0xFF101114)
val SurfaceColor = Color(0xFF1A1C20)
val SurfaceHigh = Color(0xFF24272C)
val Accent = Color(0xFFE5A00D)
val OnSurface = Color(0xFFE8E8EA)
val Muted = Color(0xFF9A9CA3)
val Danger = Color(0xFFE5534B)

/** Sizes that differ between a 10-foot TV UI and a handheld one. */
data class Dimens(
    val isTv: Boolean,
    val gutter: Dp,
    val posterWidth: Dp,
    val railSpacing: Dp,
    val focusScale: Float,
)

private val PhoneDimens = Dimens(isTv = false, gutter = 16.dp, posterWidth = 112.dp, railSpacing = 10.dp, focusScale = 1f)
private val TvDimens = Dimens(isTv = true, gutter = 48.dp, posterWidth = 140.dp, railSpacing = 16.dp, focusScale = 1.08f)

val LocalDimens = staticCompositionLocalOf { PhoneDimens }

@Composable
fun AlwyTheme(
    isTv: Boolean,
    content: @Composable () -> Unit,
) {
    val colors =
        darkColorScheme(
            primary = Accent,
            onPrimary = Color.Black,
            background = Background,
            onBackground = OnSurface,
            surface = SurfaceColor,
            onSurface = OnSurface,
            surfaceVariant = SurfaceHigh,
            onSurfaceVariant = Muted,
            error = Danger,
        )
    val base = Typography()
    // TV text is read from 3 m away: scale everything up a notch.
    val type =
        if (isTv) {
            base.copy(
                titleLarge = base.titleLarge.copy(fontSize = 28.sp),
                titleMedium = base.titleMedium.copy(fontSize = 22.sp),
                bodyLarge = base.bodyLarge.copy(fontSize = 20.sp),
                bodyMedium = base.bodyMedium.copy(fontSize = 18.sp),
                labelLarge = base.labelLarge.copy(fontSize = 18.sp),
            )
        } else {
            base
        }
    MaterialTheme(colorScheme = colors, typography = type) {
        CompositionLocalProvider(
            LocalDimens provides if (isTv) TvDimens else PhoneDimens,
            LocalContentColor provides OnSurface,
            content = content,
        )
    }
}
