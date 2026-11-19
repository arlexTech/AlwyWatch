// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind
import be.alwyapps.watch.player.DeviceProfiles
import be.alwyapps.watch.player.PlayerActivity
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.ErrorBanner
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.RemoteImage
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.backdrop
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.poster
import be.alwyapps.watch.ui.rememberSwr
import be.alwyapps.watch.ui.subtitle
import kotlinx.coroutines.launch

fun play(
    ctx: Context,
    account: String,
    item: Item,
    fromStart: Boolean,
) {
    ctx.startActivity(PlayerActivity.intent(ctx, account, item.id, if (fromStart) 0L else item.user.positionMs.toLong()))
}

@Composable
fun DetailScreen(itemId: String) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    val res =
        rememberSwr(
            account,
            itemId,
            cached = { core.cachedItem(account, itemId)?.item },
            refresh = { core.refreshItem(account, itemId, it).item },
        )
    val item = res.value
    val playable = item != null && (item.kind == ItemKind.MOVIE || item.kind == ItemKind.EPISODE)
    // Warm up playback so pressing Play starts instantly.
    if (playable) {
        LaunchedEffect(itemId) { core.prefetchPlaybackInfo(account, itemId, DeviceProfiles.get(ctx)) }
    }
    val screenH = LocalConfiguration.current.screenHeightDp.dp

    LazyColumn(Modifier.fillMaxSize()) {
        item(key = "hero") {
            Box(Modifier.fillMaxWidth().height(screenH * 0.55f)) {
                if (item != null) {
                    RemoteImage(item.id, item.backdrop() ?: item.poster(), 1280.dp, Modifier.fillMaxSize())
                }
                Box(Modifier.fillMaxSize().background(Brush.verticalGradient(listOf(Color.Transparent, Background))))
                Row(Modifier.align(Alignment.BottomStart).padding(d.gutter), verticalAlignment = Alignment.Bottom) {
                    if (item != null && !d.isTv) {
                        RemoteImage(item.id, item.poster(), 110.dp, Modifier.width(110.dp).aspectRatio(2f / 3f))
                    }
                    Column(Modifier.padding(start = if (d.isTv) 0.dp else 16.dp)) {
                        Text(
                            item?.title ?: "",
                            style = MaterialTheme.typography.headlineMedium,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                        if (item != null) {
                            val meta =
                                listOfNotNull(
                                    item.subtitle().ifEmpty { null },
                                    item.officialRating,
                                    item.communityRating?.let { "★ %.1f".format(it) },
                                )
                            Text(meta.joinToString("  ·  "), color = Muted)
                        }
                    }
                }
            }
        }
        res.error?.let { msg -> item { ErrorBanner(msg) { res.reload() } } }
        if (item != null) {
            item(key = "actions") {
                Row(Modifier.padding(horizontal = d.gutter), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (playable) {
                        val resumable = item.user.positionMs > 0u
                        ActionButton(
                            if (resumable) "Resume" else "Play",
                            { play(ctx, account, item, fromStart = false) },
                            Modifier.initialFocus(),
                            primary = true,
                        )
                        if (resumable) ActionButton("From start", { play(ctx, account, item, fromStart = true) })
                    }
                    ActionButton(if (item.user.played) "Mark unwatched" else "Mark watched", {
                        scope.launch {
                            runCatching { core.setPlayed(account, item.id, !item.user.played) }
                            res.reload()
                        }
                    })
                }
            }
            item(key = "overview") {
                Column(Modifier.padding(d.gutter)) {
                    item.overview?.let { Text(it, style = MaterialTheme.typography.bodyLarge, maxLines = if (d.isTv) 4 else 10) }
                    if (item.genres.isNotEmpty()) Text(item.genres.joinToString(", "), Modifier.padding(top = 8.dp), color = Muted)
                }
            }
            if (item.kind == ItemKind.SERIES) item(key = "seasons") { Seasons(item) }
        }
    }
}

@Composable
private fun Seasons(series: Item) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val d = LocalDimens.current
    val seasons =
        rememberSwr(account, "seasons", series.id, cached = { core.cachedSeasons(account, series.id)?.items }, refresh = {
            core.refreshSeasons(account, series.id, it).items
        })
    val list = seasons.value ?: return
    if (list.isEmpty()) return
    // Open the first season with unwatched episodes.
    var selected by remember(list) { mutableStateOf(list.firstOrNull { !it.user.played } ?: list.first()) }
    LazyRow(contentPadding = PaddingValues(horizontal = d.gutter), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        items(list, key = { it.id }) { s -> ActionButton(s.title, { selected = s }, primary = s.id == selected.id) }
    }
    Episodes(series.id, selected.id)
}

@Composable
private fun Episodes(
    seriesId: String,
    seasonId: String,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val eps =
        rememberSwr(account, "eps", seasonId, cached = { core.cachedEpisodes(account, seriesId, seasonId)?.items }, refresh = {
            core.refreshEpisodes(account, seriesId, seasonId, it).items
        })
    Column(Modifier.padding(horizontal = d.gutter, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        for (ep in eps.value.orEmpty()) {
            FocusCard(
                onClick = { play(ctx, account, ep, fromStart = false) },
                modifier =
                    Modifier.fillMaxWidth().onFocusChanged {
                        if (it.isFocused) core.prefetchPlaybackInfo(account, ep.id, DeviceProfiles.get(ctx))
                    },
            ) {
                Row(Modifier.fillMaxWidth().background(SurfaceColor).padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    RemoteImage(ep.id, ep.poster(), 200.dp, Modifier.width(if (d.isTv) 220.dp else 140.dp).aspectRatio(16f / 9f))
                    Column(Modifier.padding(start = 12.dp).weight(1f)) {
                        Text(
                            "${ep.episodeNumber ?: ""}. ${ep.title}",
                            style = MaterialTheme.typography.titleMedium,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        ep.overview?.let {
                            Text(
                                it,
                                color = Muted,
                                maxLines = 2,
                                overflow = TextOverflow.Ellipsis,
                                style = MaterialTheme.typography.bodySmall,
                            )
                        }
                    }
                    if (ep.user.played) Text("✓", Modifier.padding(horizontal = 12.dp), color = Accent)
                }
            }
        }
    }
}
