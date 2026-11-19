// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.media.ImageKind
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind

private val CardShape = RoundedCornerShape(8.dp)

/**
 * Clickable surface with a strong, visible focus state (accent ring + slight scale on TV).
 * `clickable` handles D-pad center / Enter, so the same composable works with touch and remotes.
 */
@Composable
fun FocusCard(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    shape: RoundedCornerShape = CardShape,
    content: @Composable () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val scale = if (focused) LocalDimens.current.focusScale else 1f
    Box(
        modifier
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }.clip(shape)
            .border(if (focused) 3.dp else 0.dp, if (focused) Accent else Color.Transparent, shape)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick),
    ) { content() }
}

fun Item.poster() = images.firstOrNull { it.kind == ImageKind.PRIMARY }

fun Item.backdrop() = images.firstOrNull { it.kind == ImageKind.BACKDROP }

fun Item.subtitle(): String =
    when (kind) {
        ItemKind.EPISODE ->
            listOfNotNull(
                seriesTitle,
                if (seasonNumber != null && episodeNumber != null) "S${seasonNumber}E$episodeNumber" else null,
            ).joinToString(" · ")
        else -> listOfNotNull(year?.toString(), runtimeMs?.let { "${(it / 60_000u)} min" }).joinToString(" · ")
    }

@Composable
fun PosterCard(
    item: Item,
    width: Dp,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Column(modifier.width(width)) {
        FocusCard(onClick = onClick) {
            Box {
                RemoteImage(item.id, item.poster(), width, Modifier.fillMaxWidth().aspectRatio(2f / 3f), contentDescription = item.title)
                val runtime = item.runtimeMs?.toLong() ?: 0L
                val pos = item.user.positionMs.toLong()
                if (pos > 0 && runtime > 0) {
                    Box(
                        Modifier
                            .align(Alignment.BottomStart)
                            .fillMaxWidth()
                            .height(4.dp)
                            .background(Color.Black.copy(alpha = 0.6f)),
                    )
                    Box(
                        Modifier
                            .align(Alignment.BottomStart)
                            .fillMaxWidth((pos.toFloat() / runtime).coerceIn(0f, 1f))
                            .height(4.dp)
                            .background(Accent),
                    )
                } else if (item.user.played) {
                    Text(
                        "✓",
                        Modifier
                            .align(
                                Alignment.TopEnd,
                            ).padding(6.dp)
                            .background(Accent, RoundedCornerShape(50))
                            .padding(horizontal = 6.dp),
                        color = Color.Black,
                    )
                }
            }
        }
        Text(
            if (item.kind == ItemKind.EPISODE) item.seriesTitle ?: item.title else item.title,
            Modifier.padding(top = 6.dp),
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(item.subtitle(), style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 1)
    }
}

@Composable
fun Rail(
    title: String,
    items: List<Item>,
    onClick: (Item) -> Unit,
) {
    if (items.isEmpty()) return
    val d = LocalDimens.current
    Column(Modifier.padding(top = 20.dp)) {
        Text(title, Modifier.padding(horizontal = d.gutter), style = MaterialTheme.typography.titleMedium)
        LazyRow(
            contentPadding = PaddingValues(horizontal = d.gutter, vertical = 10.dp),
            horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        ) {
            items(items, key = { it.id }) { PosterCard(it, d.posterWidth, onClick = { onClick(it) }) }
        }
    }
}

/** Grey blocks shown only on a first-ever load (never when cached data exists). */
@Composable
fun SkeletonRail() {
    val d = LocalDimens.current
    Column(Modifier.padding(top = 20.dp, start = d.gutter)) {
        Box(
            Modifier
                .width(160.dp)
                .height(20.dp)
                .clip(CardShape)
                .background(SurfaceHigh),
        )
        Row(Modifier.padding(top = 10.dp), horizontalArrangement = Arrangement.spacedBy(d.railSpacing)) {
            repeat(6) {
                Box(
                    Modifier
                        .width(d.posterWidth)
                        .aspectRatio(2f / 3f)
                        .clip(CardShape)
                        .background(SurfaceColor),
                )
            }
        }
    }
}

@Composable
fun ErrorBanner(
    message: String,
    onRetry: () -> Unit,
) {
    val d = LocalDimens.current
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = d.gutter, vertical = 8.dp)
            .clip(CardShape)
            .background(SurfaceHigh)
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(message, Modifier.weight(1f), color = Muted, style = MaterialTheme.typography.bodyMedium, maxLines = 2)
        FocusCard(onClick = onRetry) { Text("Retry", Modifier.padding(horizontal = 12.dp, vertical = 6.dp), color = Accent) }
    }
}

/**
 * Lets D-pad up/down leave a text field (Compose text fields otherwise keep focus and move the
 * cursor), which is required to reach the buttons below a field with a remote.
 */
@Composable
fun Modifier.dpadEscape(): Modifier {
    val fm = LocalFocusManager.current
    return onPreviewKeyEvent {
        if (it.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
        when (it.key) {
            Key.DirectionDown -> fm.moveFocus(FocusDirection.Down)
            Key.DirectionUp -> fm.moveFocus(FocusDirection.Up)
            else -> false
        }
    }
}

/** Requests focus once on first composition (TV: put the remote on the main action). */
@Composable
fun Modifier.initialFocus(enabled: Boolean = LocalDimens.current.isTv): Modifier {
    if (!enabled) return this
    val fr = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { fr.requestFocus() } }
    return focusRequester(fr)
}

/** Text button that is clearly focusable on TV. */
@Composable
fun ActionButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
) {
    FocusCard(onClick = onClick, modifier = modifier, shape = RoundedCornerShape(50)) {
        Text(
            label,
            Modifier.background(if (primary) Accent else SurfaceHigh).padding(horizontal = 20.dp, vertical = 10.dp),
            color = if (primary) Color.Black else OnSurface,
            style = MaterialTheme.typography.labelLarge,
        )
    }
}
