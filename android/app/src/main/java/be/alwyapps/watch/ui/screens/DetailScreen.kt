// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.downloads.DownloadState
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind
import be.alwyapps.watch.player.DeviceProfiles
import be.alwyapps.watch.player.PlayerActivity
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.ErrorBanner
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.OnSurface
import be.alwyapps.watch.ui.Outline
import be.alwyapps.watch.ui.RemoteImage
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.ambient
import be.alwyapps.watch.ui.ambientGlow
import be.alwyapps.watch.ui.backdrop
import be.alwyapps.watch.ui.fadeEdges
import be.alwyapps.watch.ui.formatRuntime
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.poster
import be.alwyapps.watch.ui.progress
import be.alwyapps.watch.ui.remaining
import be.alwyapps.watch.ui.rememberSwr
import be.alwyapps.watch.ui.thumb
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
fun DetailScreen(
    itemId: String,
    season: String? = null,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val nav = LocalNavigator.current
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
    // Shows: the episode to continue with (first unwatched, or the one in progress).
    var nextEp by remember(itemId) { mutableStateOf<Item?>(null) }
    if (item?.kind == ItemKind.SERIES) {
        LaunchedEffect(itemId) {
            // The server's own "continue watching" / "next up" know best (cached with home).
            val home = runCatching { core.cachedHome(account) }.getOrNull()
            nextEp = home?.let { h -> (h.resume + h.nextUp).firstOrNull { it.seriesId == itemId } }
            if (nextEp != null) return@LaunchedEffect
            nextEp =
                runCatching {
                    val seasons =
                        core.cachedSeasons(account, itemId)?.items ?: core.refreshSeasons(account, itemId, false).items
                    for (season in seasons.filter { !it.user.played }.ifEmpty { seasons.take(1) }) {
                        val eps =
                            core.cachedEpisodes(account, itemId, season.id)?.items
                                ?: core.refreshEpisodes(account, itemId, season.id, false).items
                        val ep = eps.firstOrNull { it.user.positionMs > 0u && !it.user.played } ?: eps.firstOrNull { !it.user.played }
                        if (ep != null) return@runCatching ep
                    }
                    null
                }.getOrNull()
        }
    }
    // Season shown in the episode list (a show page opened from an episode's "Season N").
    var selectedSeason by remember(itemId) { mutableStateOf<Item?>(null) }
    var sheet by remember { mutableStateOf(false) }
    val dls = if (d.isTv) emptyMap() else rememberDownloadMap()
    if (sheet && item != null) {
        val show = item.showFor()
        if (show != null) {
            val seasonItem =
                if (item.kind == ItemKind.EPISODE) {
                    item.seasonId?.let { sid -> item.copy(id = sid, kind = ItemKind.SEASON, title = "Season ${item.seasonNumber ?: ""}".trim()) }
                } else {
                    selectedSeason
                }
            DownloadSheet(show, seasonItem, item.takeIf { it.kind == ItemKind.EPISODE }) { sheet = false }
        }
    }
    val config = LocalConfiguration.current
    val heroH = if (d.isTv) (config.screenHeightDp * 0.8f).dp else (config.screenWidthDp * 1.2f).coerceAtMost(560f).dp

    Box(Modifier.fillMaxSize().ambientGlow(item?.ambient())) {
        LazyColumn(Modifier.fillMaxSize()) {
            item(key = "hero") {
                Box(Modifier.fillMaxWidth().height(heroH)) {
                    if (item != null) {
                        RemoteImage(
                            item.id,
                            item.backdrop() ?: item.poster(),
                            1280.dp,
                            Modifier.fillMaxSize().fadeEdges(bottomFrom = 0.5f, leftTo = if (d.isTv) 0.45f else 0f),
                            background = Color.Transparent,
                        )
                    }
                    // Top: status bar legibility. Bottom: the title and buttons stay readable on
                    // bright artwork.
                    Box(
                        Modifier.fillMaxSize().background(
                            Brush.verticalGradient(
                                0f to Color.Black.copy(alpha = 0.5f),
                                0.2f to Color.Transparent,
                                0.45f to Color.Transparent,
                                1f to
                                    be.alwyapps.watch.ui.Background
                                        .copy(alpha = 0.9f),
                            ),
                        ),
                    )
                    if (item != null) {
                        Column(
                            Modifier
                                .align(Alignment.BottomStart)
                                .fillMaxWidth(if (d.isTv) 0.55f else 1f)
                                .padding(horizontal = d.gutter, vertical = if (d.isTv) 40.dp else 8.dp),
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            if (item.kind == ItemKind.EPISODE) {
                                // The show and the season, one tap away.
                                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    item.seriesId?.let { sid ->
                                        ActionButton(item.seriesTitle ?: "Show", { nav.push(Route.Detail(sid)) }, icon = Icons.Tv)
                                        if (item.seasonId != null && item.seasonNumber != null) {
                                            ActionButton("Season ${item.seasonNumber}", { nav.push(Route.Detail(sid, item.seasonId)) })
                                        }
                                    }
                                }
                                Text(item.title, style = MaterialTheme.typography.displaySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            } else {
                                TitleArt(item, if (d.isTv) 460.dp else 300.dp, big = d.isTv)
                            }
                            HeroMeta(item)
                            Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                                if (playable) {
                                    val resumable = item.user.positionMs > 0u
                                    ActionButton(
                                        if (resumable) "Resume" else "Play",
                                        { play(ctx, account, item, fromStart = false) },
                                        Modifier.initialFocus(),
                                        primary = true,
                                        icon = Icons.Play,
                                    )
                                    if (resumable) ActionButton("From start", { play(ctx, account, item, fromStart = true) })
                                }
                                nextEp?.let { ep ->
                                    val label = "S${ep.seasonNumber ?: 1} · E${ep.episodeNumber ?: 1}"
                                    ActionButton(
                                        if (ep.user.positionMs > 0u) "Resume $label" else "Play $label",
                                        { play(ctx, account, ep, fromStart = false) },
                                        Modifier.initialFocus(),
                                        primary = true,
                                        icon = Icons.Play,
                                    )
                                }
                                IconAction(
                                    Icons.Check,
                                    if (item.user.played) "Mark unwatched" else "Mark watched",
                                    {
                                        scope.launch {
                                            runCatching { core.setPlayed(account, item.id, !item.user.played) }
                                            res.reload()
                                        }
                                    },
                                    active = item.user.played,
                                )
                                if (!d.isTv) {
                                    when (item.kind) {
                                        ItemKind.MOVIE -> DownloadButton(item, dls[item.id])
                                        // Episode or show: this one, its season, the whole show, next N.
                                        ItemKind.EPISODE -> DownloadStateButton(dls[item.id], { sheet = true })
                                        ItemKind.SERIES -> {
                                            val mine = dls.values.filter { it.episode?.seriesId == item.id }
                                            DownloadStateButton(
                                                mine.firstOrNull { it.state != DownloadState.COMPLETED } ?: mine.firstOrNull(),
                                                { sheet = true },
                                            )
                                        }
                                        else -> {}
                                    }
                                }
                            }
                            val downloadNote =
                                when (item.kind) {
                                    ItemKind.SERIES -> {
                                        val mine = dls.values.filter { it.episode?.seriesId == item.id }
                                        val done = mine.filter { it.state == DownloadState.COMPLETED }
                                        val running = mine.size - done.size
                                        listOfNotNull(
                                            if (done.isNotEmpty()) {
                                                "${done.size} downloaded · %.1f GB".format(done.sumOf { (it.total ?: it.downloaded).toLong() } / 1e9)
                                            } else {
                                                null
                                            },
                                            if (running > 0) "$running downloading" else null,
                                        ).joinToString(" · ").ifEmpty { null }
                                    }
                                    else -> dls[item.id]?.summary()
                                }
                            listOfNotNull(item.remaining(), downloadNote).joinToString("  ·  ").ifEmpty { null }?.let {
                                Text(it, style = MaterialTheme.typography.bodySmall, color = Muted)
                            }
                        }
                    }
                    if (!d.isTv) {
                        IconAction(
                            Icons.Back,
                            "Back",
                            { nav.pop() },
                            Modifier.align(Alignment.TopStart).statusBarsPadding().padding(12.dp),
                        )
                    }
                }
            }
            res.error?.let { msg -> item { ErrorBanner(msg) { res.reload() } } }
            if (item != null) {
                item(key = "overview") {
                    Column(
                        Modifier.padding(horizontal = d.gutter, vertical = 12.dp).fillMaxWidth(if (d.isTv) 0.6f else 1f),
                        verticalArrangement = Arrangement.spacedBy(14.dp),
                    ) {
                        item.overview?.let {
                            Text(it, style = MaterialTheme.typography.bodyLarge, color = OnSurface.copy(alpha = 0.85f), maxLines = if (d.isTv) 4 else 8)
                        }
                        if (item.genres.isNotEmpty()) {
                            androidx.compose.foundation.layout.FlowRow(
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                                verticalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                for (g in item.genres.take(5)) Chip(g)
                            }
                        }
                    }
                }
                if (item.kind == ItemKind.SERIES) item(key = "seasons") { Seasons(item, season, dls) { selectedSeason = it } }
            }
            item { Spacer(Modifier.height(40.dp)) }
        }
    }
}

@Composable
private fun Chip(text: String) {
    Text(
        text,
        Modifier
            .clip(RoundedCornerShape(50))
            .border(1.dp, Outline, RoundedCornerShape(50))
            .padding(horizontal = 12.dp, vertical = 6.dp),
        style = MaterialTheme.typography.labelMedium,
        color = Muted,
        maxLines = 1,
    )
}

/**
 * Download toggle for a movie or an episode: downloads it (default deletion rule); while it
 * downloads, tapping cancels; once downloaded, tapping asks before removing it.
 */
@Composable
private fun DownloadButton(
    item: Item,
    dl: be.alwyapps.watch.core.downloads.Download?,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var confirm by remember { mutableStateOf(false) }
    val permission =
        androidx.activity.compose.rememberLauncherForActivityResult(
            androidx.activity.result.contract.ActivityResultContracts
                .RequestPermission(),
        ) { }
    if (confirm && dl != null) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirm = false },
            title = { Text("Remove the download?") },
            text = { Text("${item.title} (${dl.progressLabel()}) will be deleted from this phone.") },
            confirmButton = {
                ActionButton("Remove", {
                    core.removeDownload(dl.id)
                    confirm = false
                }, primary = true)
            },
            dismissButton = { ActionButton("Keep", { confirm = false }) },
            containerColor = be.alwyapps.watch.ui.Background,
        )
    }
    DownloadStateButton(dl, {
        when {
            dl == null -> {
                if (android.os.Build.VERSION.SDK_INT >= 33) permission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                scope.launch {
                    runCatching { core.downloadItem(account, item.id, null) }
                    startDownloads(ctx)
                }
            }
            dl.state == DownloadState.COMPLETED -> confirm = true
            else -> core.removeDownload(dl.id)
        }
    })
}

@Composable
private fun Seasons(
    series: Item,
    initial: String?,
    dls: Map<String, be.alwyapps.watch.core.downloads.Download>,
    onSelected: (Item) -> Unit,
) {
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
    var selected by remember(list) {
        mutableStateOf(list.firstOrNull { it.id == initial } ?: list.firstOrNull { !it.user.played } ?: list.first())
    }
    LaunchedEffect(selected) { onSelected(selected) }
    Text("Episodes", Modifier.padding(horizontal = d.gutter, vertical = 8.dp), style = MaterialTheme.typography.titleMedium)
    LazyRow(contentPadding = PaddingValues(horizontal = d.gutter), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        items(list, key = { it.id }) { s -> ActionButton(s.title, { selected = s }, primary = s.id == selected.id) }
    }
    Episodes(series.id, selected.id, dls)
}

@Composable
private fun Episodes(
    seriesId: String,
    seasonId: String,
    dls: Map<String, be.alwyapps.watch.core.downloads.Download>,
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
                Row(Modifier.fillMaxWidth().padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.width(if (d.isTv) 260.dp else 150.dp).clip(RoundedCornerShape(d.radius - 4.dp))) {
                        RemoteImage(ep.id, ep.thumb(), 300.dp, Modifier.fillMaxWidth().aspectRatio(16f / 9f))
                        ep.progress()?.let {
                            Box(
                                Modifier
                                    .align(Alignment.BottomStart)
                                    .fillMaxWidth(it)
                                    .height(3.dp)
                                    .background(Accent),
                            )
                        }
                        if (ep.user.played) {
                            Icon(
                                Icons.Check,
                                "Watched",
                                Modifier
                                    .align(Alignment.TopEnd)
                                    .padding(6.dp)
                                    .size(20.dp)
                                    .clip(RoundedCornerShape(50))
                                    .background(Color.Black.copy(alpha = 0.6f))
                                    .padding(3.dp),
                                tint = Accent,
                            )
                        }
                    }
                    Column(Modifier.padding(start = 14.dp).weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        Text(
                            "${ep.episodeNumber ?: ""}. ${ep.title}",
                            style = MaterialTheme.typography.titleSmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            listOfNotNull(
                                ep.runtimeMs?.let { formatRuntime(it.toLong()) },
                                ep.remaining(),
                                dls[ep.id]?.let { if (it.state == DownloadState.COMPLETED) it.progressLabel() else "downloading ${it.progressLabel()}" },
                            ).joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = Subtle,
                        )
                        ep.overview?.let {
                            Text(it, color = Muted, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                    // Phones/tablets: download an episode straight from the list.
                    if (!d.isTv) DownloadButton(ep, dls[ep.id])
                }
            }
        }
    }
}
