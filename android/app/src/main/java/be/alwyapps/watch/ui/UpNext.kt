// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.UpNextMovie

/**
 * What to watch after a movie, with its poster: the next film of its saga (or one like it).
 * In the player near the end (with the controls) and on the page of a watched movie.
 */
@Composable
fun UpNextCard(
    next: UpNextMovie,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val item = next.item
    FocusCard(onClick = onClick, modifier = modifier, glow = false, focusScale = 1.03f) {
        Row(
            Modifier.background(Background.copy(alpha = 0.92f)).padding(10.dp),
            horizontalArrangement = Arrangement.spacedBy(14.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.width(72.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(8.dp))) {
                val account = next.onServer?.accountId
                if (item != null && account != null) {
                    CompositionLocalProvider(LocalAccount provides account) {
                        RemoteImage(item.id, item.poster(), 72.dp, Modifier.matchParentSize(), contentDescription = next.title)
                    }
                } else {
                    UrlImage(next.posterUrl, 72.dp, Modifier.matchParentSize())
                }
            }
            Column(Modifier.widthIn(max = 240.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(
                    (if (next.saga) tr("Next in the saga") else tr("Up next")).uppercase(),
                    style = MaterialTheme.typography.labelSmall,
                    color = Accent,
                )
                Text(next.title, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                item?.let { Text(it.subtitle(), style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 1) }
                Text(
                    if (next.onServer != null) tr("Play") else tr("Not on your servers · request it"),
                    style = MaterialTheme.typography.bodySmall,
                    color = Muted,
                    maxLines = 2,
                )
            }
        }
    }
}
