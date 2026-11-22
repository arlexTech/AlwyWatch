// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusGroup
import androidx.compose.foundation.interaction.collectIsFocusedAsState
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
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.focusRestorer
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
import be.alwyapps.watch.player.PlayerSettings
import be.alwyapps.watch.services
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
import be.alwyapps.watch.ui.dpadScroll
import be.alwyapps.watch.ui.fadeEdges
import be.alwyapps.watch.ui.focusOnAppear
import be.alwyapps.watch.ui.formatRuntime
import be.alwyapps.watch.ui.poster
import be.alwyapps.watch.ui.progress
import be.alwyapps.watch.ui.remaining
import be.alwyapps.watch.ui.rememberFocus
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
    // "Fully drawn" for startup benchmarks when the app reopens on a title page (only the home
    // reported it: a cold start onto this page logged it whenever the home was reached later).
    androidx.activity.compose.ReportDrawnWhen { item != null || res.error != null }
    // TV: the action that had the focus, for coming back here; opened anew, Play.
    val focusKey = "detail.$itemId"
    remember(itemId) {
        if (!nav.returning) {
            be.alwyapps.watch.ui.FocusMemory
                .forget(focusKey)
        }
    }
    // Back from the player, or watched state changed from a menu: positions and marks moved.
    val changed = be.alwyapps.watch.ui.ItemsChanged.version
    LaunchedEffect(changed) { if (changed > 0) res.reload() }
    // Watch together goes through the AlwyWatch server.
    val togetherOn = remember { runCatching { core.keepActive() }.getOrDefault(false) }
    var creatingRoom by remember { mutableStateOf(false) }
    // My list (movies and shows), from the cache; the button flips it on the server.
    var watchlisted by remember(itemId) { mutableStateOf(core.isWatchlisted(account, itemId)) }
    LaunchedEffect(itemId) {
        runCatching { core.refreshWatchlist(account, false) }
        watchlisted = core.isWatchlisted(account, itemId)
    }
    val toggleList = {
        val on = !watchlisted
        watchlisted = on
        scope.launch {
            runCatching { core.setWatchlisted(account, itemId, on) }.onFailure {
                watchlisted = !on
                android.widget.Toast
                    .makeText(ctx, "My list: ${it.message}", android.widget.Toast.LENGTH_LONG)
                    .show()
            }
        }
        Unit
    }
    // Trailers (server extras, else web links) and, on show pages, the theme song.
    var trailers by remember(itemId) { mutableStateOf<List<be.alwyapps.watch.core.media.Trailer>>(emptyList()) }
    LaunchedEffect(itemId) { runCatching { core.trailers(account, itemId) }.onSuccess { trailers = it } }
    val playTrailer: (() -> Unit)? =
        trailers.firstOrNull()?.let { t ->
            {
                val id = t.itemId
                val url = t.url
                if (id != null) {
                    ctx.startActivity(PlayerActivity.intent(ctx, account, id, 0L))
                } else if (url != null) {
                    runCatching { ctx.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url))) }
                }
            }
        }
    ThemeMusic(account, itemId, item?.kind == ItemKind.SERIES)
    // Cast and crew: cached first, then the server's.
    var people by remember(itemId) { mutableStateOf(core.cachedPeople(account, itemId).orEmpty()) }
    LaunchedEffect(itemId) { runCatching { core.people(account, itemId) }.onSuccess { people = it } }
    // A season has no page of its own: its show, opened on that season.
    val parent = item?.seriesId
    if (item?.kind == ItemKind.SEASON && parent != null) {
        LaunchedEffect(item.id) {
            nav.pop()
            nav.push(Route.Detail(parent, item.id, account))
        }
        return
    }
    val playable = item != null && (item.kind == ItemKind.MOVIE || item.kind == ItemKind.EPISODE)
    // Warm up playback so pressing Play starts instantly.
    if (playable) {
        LaunchedEffect(itemId) { core.prefetchPlaybackInfo(account, itemId, DeviceProfiles.load(ctx)) }
    }
    // Shows: the episode to continue with (first unwatched, or the one in progress).
    var nextEp by remember(itemId) { mutableStateOf<Item?>(null) }
    if (item?.kind == ItemKind.SERIES) {
        LaunchedEffect(itemId, changed) {
            // After a change the caches are stale: ask the server (cheap, cached again after).
            val fresh = changed > 0
            // The server's own "continue watching" / "next up" know best (cached with home).
            val home = runCatching { if (fresh) core.refreshHome(account, false) else core.cachedHome(account) }.getOrNull()
            // Assigned once at the end: a null in between would remove the Play button for a
            // moment, and its return would pull the focus back to it.
            val fromHome = home?.let { h -> h.continueWatching.firstOrNull { it.seriesId == itemId } }
            if (fromHome != null) {
                nextEp = fromHome
                return@LaunchedEffect
            }
            nextEp =
                runCatching {
                    val seasons =
                        (if (fresh) null else core.cachedSeasons(account, itemId)?.items) ?: core.refreshSeasons(account, itemId, false).items
                    for (season in seasons.filter { !it.user.played }.ifEmpty { seasons.take(1) }) {
                        val eps =
                            (if (fresh) null else core.cachedEpisodes(account, itemId, season.id)?.items)
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
    // Shows: seasons and the shown season's episodes, each episode its own lazy row (a long
    // season was one block: all its cards and images built and drawn at once).
    val seasonList =
        if (item?.kind == ItemKind.SERIES) {
            rememberSwr(account, "seasons", item.id, cached = { core.cachedSeasons(account, item.id)?.items }, refresh = {
                core.refreshSeasons(account, item.id, it).items
            })
                .also { s -> LaunchedEffect(changed) { if (changed > 0) s.reload() } }
                .value
                .orEmpty()
        } else {
            emptyList()
        }
    // The season picked here survives a refresh of the list (it went back to the default).
    var pickedSeason by remember(itemId) { mutableStateOf(season) }
    val shownSeason =
        seasonList.firstOrNull { it.id == pickedSeason } ?: seasonList.firstOrNull { !it.user.played } ?: seasonList.firstOrNull()
    LaunchedEffect(shownSeason?.id) { selectedSeason = shownSeason }
    val episodes =
        if (item != null && shownSeason != null) {
            rememberSwr(account, "eps", shownSeason.id, cached = { core.cachedEpisodes(account, item.id, shownSeason.id)?.items }, refresh = {
                core.refreshEpisodes(account, item.id, shownSeason.id, it).items
            }).also { eps -> LaunchedEffect(changed) { if (changed > 0) eps.reload() } }
        } else {
            null
        }
    // "More seasons": the show's seasons in Seerr, looked up once the page is shown (never
    // delays it). Nothing shows while it loads, or if Seerr can't be reached.
    val showTmdb =
        item
            ?.takeIf { it.kind == ItemKind.SERIES }
            ?.providerIds
            ?.tmdb
            ?.toLongOrNull()
    val seerrOn = remember { runCatching { core.seerrActive() }.getOrDefault(false) }
    var seerrSeasons by remember(itemId) { mutableStateOf<List<be.alwyapps.watch.core.seerr.SeerrSeasonInfo>?>(null) }
    if (seerrOn && showTmdb != null) {
        LaunchedEffect(showTmdb) { seerrSeasons = runCatching { core.seerrSeasonInfo(showTmdb) }.getOrNull() }
    }
    val haveSeasons = seasonList.mapNotNull { it.seasonNumber }
    val canRequestMore =
        remember(seerrSeasons, haveSeasons) {
            seerrSeasons?.let {
                be.alwyapps.watch.core
                    .seasonPlan(it, haveSeasons)
                    .missing
                    .isNotEmpty()
            } == true
        }
    var requestingSeasons by remember { mutableStateOf(false) }
    val moreDirect = remember { booleanArrayOf(false) }
    val moreFocus =
        remember {
            androidx.compose.ui.focus
                .FocusRequester()
        }
    // A whole show or season watched / unwatched (asks before forgetting where you stopped).
    var markAll by remember { mutableStateOf<Item?>(null) }
    val markFocus =
        remember {
            androidx.compose.ui.focus
                .FocusRequester()
        }
    val seasonMarkFocus =
        remember {
            androidx.compose.ui.focus
                .FocusRequester()
        }
    markAll?.let { target ->
        val isShow = target.kind == ItemKind.SERIES
        be.alwyapps.watch.ui.MarkAllDialog(
            target,
            if (isShow) target.title else "${item?.title ?: ""} · ${target.title}".trimStart(' ', '·'),
            if (isShow) seasonList.sumOf { (it.childCount ?: 0u).toInt() }.takeIf { it > 0 } else target.childCount?.toInt(),
            onDismiss = {
                markAll = null
                // TV: back on what opened it.
                if (d.isTv) scope.launch { runCatching { (if (isShow) markFocus else seasonMarkFocus).requestFocus() } }
            },
        )
    }
    val requestInfo = seerrSeasons
    if (requestingSeasons && showTmdb != null && requestInfo != null) {
        RequestSeasonsDialog(showTmdb, item?.title.orEmpty(), haveSeasons, requestInfo) { latest ->
            requestingSeasons = false
            seerrSeasons = latest
            // TV: back on "More seasons", or on the season row's mark when nothing is left.
            if (d.isTv) {
                val left =
                    be.alwyapps.watch.core
                        .seasonPlan(latest, haveSeasons)
                        .missing
                        .isNotEmpty()
                scope.launch {
                    if (left) moreDirect[0] = true
                    runCatching { (if (left) moreFocus else seasonMarkFocus).requestFocus() }
                    moreDirect[0] = false
                }
            }
        }
    }
    var episodeMenu by remember { mutableStateOf<Item?>(null) }
    episodeMenu?.let {
        be.alwyapps.watch.ui
            .ItemMenu(it) { episodeMenu = null }
    }
    val config = LocalConfiguration.current
    val heroH = if (d.isTv) (config.screenHeightDp * 0.66f).dp else (config.screenWidthDp * 1.2f).coerceAtMost(560f).dp

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
                                .fillMaxWidth(if (d.isTv) 0.75f else 1f)
                                .padding(horizontal = d.gutter, vertical = if (d.isTv) 24.dp else 8.dp),
                            verticalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            if (item.kind == ItemKind.EPISODE) {
                                // The show first (like Plex): its name or logo, leading to the
                                // episode's season. The episode's own title comes second.
                                val sid = item.seriesId
                                Row(
                                    Modifier.then(
                                        if (sid != null) Modifier.clickable { nav.push(Route.Detail(sid, item.seasonId, account)) } else Modifier,
                                    ),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    Box(Modifier.weight(1f, fill = false)) { TitleArt(item, if (d.isTv) 340.dp else 280.dp) }
                                    if (sid != null) Icon(Icons.ChevronRight, "Open the show", Modifier.size(32.dp), tint = Muted)
                                }
                                Text(item.title, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            } else {
                                TitleArt(item, if (d.isTv) 340.dp else 300.dp)
                            }
                            HeroMeta(item)
                            // What Play starts: this movie/episode, or the show's episode to continue.
                            val target: Item? = if (playable) item else nextEp
                            val resumable = target != null && target.user.positionMs > 0u
                            val at =
                                target?.let {
                                    be.alwyapps.watch.player
                                        .formatTime(it.user.positionMs.toLong())
                                }
                            val epLabel = target?.takeIf { !playable }?.let { "S${it.seasonNumber ?: 1} · E${it.episodeNumber ?: 1}" }
                            val playLabel: String? =
                                when {
                                    target == null -> null
                                    resumable -> listOfNotNull("Resume", epLabel, "from $at").joinToString(" ")
                                    else -> listOfNotNull("Play", epLabel).joinToString(" ")
                                }
                            val playAction: (() -> Unit)? = target?.let { t -> { play(ctx, account, t, fromStart = false) } }
                            val fromStart: (() -> Unit)? = target?.takeIf { resumable }?.let { t -> { play(ctx, account, t, fromStart = true) } }
                            // Opens a private room with it (never the player): the host invites, then starts.
                            val watchTogether: (() -> Unit)? =
                                target?.takeIf { togetherOn }?.let { t ->
                                    {
                                        if (!creatingRoom) {
                                            creatingRoom = true
                                            scope.launch {
                                                try {
                                                    createRoom(core, nav, account, t.id, t.user.positionMs.toLong())
                                                } catch (e: be.alwyapps.watch.core.CoreException) {
                                                    android.widget.Toast
                                                        .makeText(
                                                            ctx,
                                                            be.alwyapps.watch.together
                                                                .friendly(e),
                                                            android.widget.Toast.LENGTH_LONG,
                                                        ).show()
                                                } finally {
                                                    creatingRoom = false
                                                }
                                            }
                                        }
                                    }
                                }
                            val toggleWatched = {
                                if (item.kind == ItemKind.SERIES) {
                                    markAll = item
                                } else {
                                    scope.launch {
                                        runCatching { core.setPlayed(account, item.id, !item.user.played) }
                                        res.reload()
                                    }
                                }
                                Unit
                            }
                            if (d.isTv) {
                                // Past the column if needed: the focused icon spells out its label.
                                Row(
                                    Modifier.wrapContentWidth(Alignment.Start, unbounded = true),
                                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    // Back on this page (from a room left, the trailer…): the button
                                    // that opened it has the focus again; opened anew, Play.
                                    if (playLabel != null && playAction != null) {
                                        ActionButton(
                                            playLabel,
                                            playAction,
                                            Modifier.rememberFocus(focusKey, "play", default = true),
                                            primary = true,
                                            icon = Icons.Play,
                                        )
                                    }
                                    fromStart?.let { ActionButton("From start", it, Modifier.rememberFocus(focusKey, "start")) }
                                    watchTogether?.let { IconAction(Icons.Group, "Watch together", it, Modifier.rememberFocus(focusKey, "together")) }
                                    playTrailer?.let { IconAction(Icons.Movie, "Trailer", it, Modifier.rememberFocus(focusKey, "trailer")) }
                                    if (item.kind == ItemKind.MOVIE || item.kind == ItemKind.SERIES) {
                                        IconAction(
                                            if (watchlisted) Icons.BookmarkFilled else Icons.Bookmark,
                                            if (watchlisted) "Remove from my list" else "Add to my list",
                                            toggleList,
                                            Modifier.rememberFocus(focusKey, "list"),
                                            active = watchlisted,
                                        )
                                    }
                                    IconAction(
                                        Icons.Check,
                                        when {
                                            item.kind == ItemKind.SERIES -> "Watched or not"
                                            item.user.played -> "Mark unwatched"
                                            else -> "Mark watched"
                                        },
                                        toggleWatched,
                                        Modifier
                                            .rememberFocus(focusKey, "watched")
                                            .then(if (item.kind == ItemKind.SERIES) Modifier.focusRequester(markFocus) else Modifier),
                                        active = item.user.played,
                                    )
                                }
                            } else {
                                // Phones, like Plex: one wide play button, then labelled actions.
                                if (playLabel != null && playAction != null) {
                                    ActionButton(playLabel, playAction, Modifier.fillMaxWidth(), primary = true, icon = Icons.Play)
                                }
                                Row(Modifier.fillMaxWidth().padding(top = 4.dp), horizontalArrangement = Arrangement.SpaceEvenly) {
                                    if (fromStart != null) {
                                        CaptionAction("From start", fromStart) {
                                            Icon(Icons.Refresh, null, Modifier.size(22.dp))
                                        }
                                    }
                                    watchTogether?.let {
                                        CaptionAction("Together", it) { Icon(Icons.Group, null, Modifier.size(22.dp)) }
                                    }
                                    playTrailer?.let {
                                        CaptionAction("Trailer", it) { Icon(Icons.Movie, null, Modifier.size(22.dp)) }
                                    }
                                    if (item.kind == ItemKind.MOVIE || item.kind == ItemKind.SERIES) {
                                        CaptionAction(if (watchlisted) "In my list" else "My list", toggleList, highlighted = watchlisted) {
                                            Icon(
                                                if (watchlisted) Icons.BookmarkFilled else Icons.Bookmark,
                                                null,
                                                Modifier.size(22.dp),
                                                tint = if (watchlisted) Color.Black else OnSurface,
                                            )
                                        }
                                    }
                                    CaptionAction(if (item.user.played) "Watched" else "Mark watched", toggleWatched, highlighted = item.user.played) {
                                        Icon(Icons.Check, null, Modifier.size(22.dp), tint = if (item.user.played) Color.Black else OnSurface)
                                    }
                                    when (item.kind) {
                                        ItemKind.MOVIE -> DownloadAction(item, dls[item.id], toggle = true) {}
                                        // Episode or show: the sheet (this one, a season, the show, next N).
                                        ItemKind.EPISODE -> DownloadAction(item, dls[item.id], toggle = false) { sheet = true }
                                        ItemKind.SERIES -> SeriesDownloadAction(item, dls.values.filter { it.episode?.seriesId == item.id }) { sheet = true }
                                        else -> {}
                                    }
                                }
                            }
                            // Version, audio and subtitles of what Play starts (like Plex), and file info.
                            target?.let { t -> TrackPills(account, t) }
                            target?.remaining()?.let { left ->
                                Text(left, style = MaterialTheme.typography.bodySmall, color = Muted)
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
                        item.overview?.let { Overview(item.title, it) }
                        if (item.genres.isNotEmpty()) {
                            androidx.compose.foundation.layout.FlowRow(
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                                verticalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                for (g in item.genres.take(5)) Chip(g)
                            }
                        }
                        // "Directed by …", like Plex.
                        people
                            .filter { it.kind == be.alwyapps.watch.core.media.PersonKind.DIRECTOR }
                            .take(2)
                            .takeIf { it.isNotEmpty() }
                            ?.let { dirs ->
                                Text(
                                    "Directed by " + dirs.joinToString(", ") { it.name },
                                    style = MaterialTheme.typography.bodySmall,
                                    color = Muted,
                                )
                            }
                    }
                }
                if (item.kind == ItemKind.SERIES && shownSeason != null) {
                    item(key = "seasons") {
                        SeasonPicker(
                            seasonList,
                            shownSeason,
                            onMark = { markAll = it },
                            markFocus = seasonMarkFocus,
                            onMore = if (canRequestMore) ({ requestingSeasons = true }) else null,
                            moreFocus = moreFocus,
                            direct = moreDirect,
                        ) { pickedSeason = it.id }
                    }
                    item(key = "eps-top") { Spacer(Modifier.height(7.dp)) }
                    items(episodes?.value.orEmpty(), key = { "ep:" + it.id }) { ep ->
                        EpisodeRow(ep, dls[ep.id]) { episodeMenu = ep }
                    }
                    item(key = "eps-end") { Spacer(Modifier.height(7.dp)) }
                }
                val cast = people.filter { it.kind == be.alwyapps.watch.core.media.PersonKind.ACTOR }.take(20)
                if (cast.isNotEmpty()) item(key = "cast") { CastRow(cast) }
            }
            item { Spacer(Modifier.height(40.dp)) }
        }
    }
}

private fun streamLanguage(st: be.alwyapps.watch.core.media.MediaStream) =
    be.alwyapps.watch.player
        .languageName(st.language) ?: st.title ?: "Unknown"

private fun audioLabel(st: be.alwyapps.watch.core.media.MediaStream): String {
    val codec = st.codec.uppercase()
    val channels =
        when (st.channels?.toInt()) {
            null, 0 -> null
            1 -> "Mono"
            2 -> "Stereo"
            6 -> "5.1"
            8 -> "7.1"
            else -> "${st.channels} ch"
        }
    // The track's own title often repeats language and codec ("French VFQ AC3 5.1"): keep only
    // what it adds ("VFQ").
    val known =
        setOfNotNull(
            codec.lowercase(),
            channels?.lowercase(),
            st.language?.lowercase(),
            be.alwyapps.watch.player
                .languageName(st.language)
                ?.lowercase(),
            st.language?.let {
                @Suppress("DEPRECATION")
                java.util
                    .Locale(
                        be.alwyapps.watch.player
                            .lang3(it) ?: it,
                    ).getDisplayLanguage(java.util.Locale.ENGLISH)
                    .lowercase()
            },
            "stereo",
            "mono",
            "audio",
            "dolby",
            "digital",
            "-",
        )
    val extra =
        st.title
            ?.split(' ', '-', '_', '/', '(', ')', '[', ']')
            ?.filter { w -> w.isNotBlank() && w.lowercase() !in known && !w.matches(Regex("\\d\\.\\d|\\d+ch", RegexOption.IGNORE_CASE)) }
            ?.joinToString(" ")
            ?.takeIf { it.isNotBlank() }
    return listOfNotNull(
        streamLanguage(st),
        listOfNotNull(codec.takeIf { it.isNotEmpty() }, channels).joinToString(" ").ifEmpty { null }?.let { "($it)" },
        extra?.let { "· $it" },
    ).joinToString(" ")
}

private fun subtitleLabel(st: be.alwyapps.watch.core.media.MediaStream): String =
    listOfNotNull(
        streamLanguage(st),
        "forced".takeIf { st.isForced },
        st.title?.takeIf {
            it.isNotBlank() &&
                be.alwyapps.watch.player
                    .languageName(st.language) != null &&
                !st.isForced
        },
    ).joinToString(" · ")

private fun videoLabel(src: be.alwyapps.watch.core.media.MediaSource): String? {
    val v = src.streams.firstOrNull { it.kind == be.alwyapps.watch.core.media.StreamKind.VIDEO } ?: return null
    // By width: a 1920×800 scope film is "1080p", not "800p".
    val w = v.width?.toInt() ?: 0
    val h = v.height?.toInt() ?: 0
    val res =
        when {
            w >= 3200 || h >= 2000 -> "4K"
            w >= 1800 || h >= 1000 -> "1080p"
            w >= 1200 || h >= 700 -> "720p"
            h > 0 -> "${h}p"
            else -> null
        }
    val hdr =
        when (v.hdr) {
            be.alwyapps.watch.core.media.HdrKind.DOLBY_VISION -> "Dolby Vision"
            be.alwyapps.watch.core.media.HdrKind.HDR10 -> "HDR10"
            be.alwyapps.watch.core.media.HdrKind.HLG -> "HLG"
            else -> null
        }
    return listOfNotNull(res, "(${listOfNotNull(v.codec.uppercase(), v.profile).joinToString(" ")})", hdr).joinToString(" ")
}

/**
 * Plex's track pills on title pages: version (when there are several), audio and subtitles for
 * what Play starts, each opening a list; the choice is kept for the movie or show. And file info.
 */
@Composable
private fun TrackPills(
    account: String,
    target: Item,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    var info by remember(target.id) { mutableStateOf<be.alwyapps.watch.core.media.PlaybackInfo?>(null) }
    LaunchedEffect(target.id) {
        info = runCatching { core.playbackInfo(account, target.id, DeviceProfiles.load(ctx)) }.getOrNull()
    }
    val pi = info ?: return
    val key =
        be.alwyapps.watch.player.TrackPrefs
            .titleKey(target)
    var version by remember(target.id) {
        mutableStateOf(
            be.alwyapps.watch.player.TrackPrefs
                .version(core, account, target.id),
        )
    }
    var pref by remember(key) {
        mutableStateOf(
            be.alwyapps.watch.player.TrackPrefs
                .get(core, account, key),
        )
    }
    val src = pi.sources.firstOrNull { it.id == version } ?: pi.sources.firstOrNull() ?: return
    val audio = src.streams.filter { it.kind == be.alwyapps.watch.core.media.StreamKind.AUDIO }
    // In the player's order: tracks in the file, then the side-loaded ones.
    val subs =
        src.streams
            .filter { it.kind == be.alwyapps.watch.core.media.StreamKind.SUBTITLE }
            .sortedBy { if (it.external == null) 0 else 1 }
    val settings = remember { PlayerSettings(core, true) }
    val p = pref

    // The server's tracks in the player's order, numbered for the core (see TrackPrefs).
    val streams = audio + subs
    val infos =
        streams.mapIndexed { i, st ->
            be.alwyapps.watch.core.TrackInfo(
                i.toUInt(),
                if (st.kind == be.alwyapps.watch.core.media.StreamKind.AUDIO) {
                    be.alwyapps.watch.core.TrackKind.AUDIO
                } else {
                    be.alwyapps.watch.core.TrackKind.SUBTITLE
                },
                st.language,
                st.title,
                st.isForced,
            )
        }

    fun remembered(kind: be.alwyapps.watch.core.TrackKind) =
        p
            ?.let {
                be.alwyapps.watch.core
                    .rememberedTrack(it, kind, infos)
            }?.let { streams[it.toInt()] }

    fun picked(
        kind: be.alwyapps.watch.core.TrackKind,
        st: be.alwyapps.watch.core.media.MediaStream?,
    ) = be.alwyapps.watch.core
        .pickTrack(pref, kind, infos, st?.let { streams.indexOf(it).toUInt() })
    val rememberedSubs = remembered(be.alwyapps.watch.core.TrackKind.SUBTITLE)
    val currentAudio =
        remembered(be.alwyapps.watch.core.TrackKind.AUDIO) ?: settings.audioLanguage.takeIf { it.isNotEmpty() }?.let { a ->
            audio.firstOrNull {
                be.alwyapps.watch.player
                    .lang3(it.language) ==
                    be.alwyapps.watch.player
                        .lang3(a)
            }
        }
            ?: audio.firstOrNull { it.index == src.defaultAudioIndex } ?: audio.firstOrNull { it.isDefault } ?: audio.firstOrNull()
    val subsText =
        when {
            p?.subs == "off" -> "Off"
            p?.subs != null ->
                rememberedSubs?.let(::subtitleLabel) ?: (
                    be.alwyapps.watch.player
                        .languageName(p.subs) ?: p.subs
                ) + if (p.forced) " · forced" else ""
            subs.isEmpty() -> "None"
            else ->
                when (settings.subtitleMode) {
                    "off" -> "Off"
                    "forced" -> "Forced only"
                    "always" ->
                        be.alwyapps.watch.player
                            .languageName(
                                settings.subtitleLanguage.ifEmpty {
                                    java.util.Locale
                                        .getDefault()
                                        .language
                                },
                            ) ?: "On"
                    else -> "Automatic"
                }
        }
    var open by remember { mutableStateOf<String?>(null) }
    androidx.compose.foundation.layout.FlowRow(
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val vLabel = videoLabel(src)
        if (pi.sources.size > 1) {
            SmallPill(Icons.Movie, vLabel ?: "Version") { open = "version" }
        } else if (vLabel != null) {
            SmallPill(Icons.Movie, vLabel) { open = "info" }
        }
        if (audio.isNotEmpty()) SmallPill(Icons.Volume, currentAudio?.let(::audioLabel) ?: "Audio") { open = "audio" }
        SmallPill(Icons.Subtitles, subsText) { open = "subs" }
        SmallPill(Icons.Info, "File info") { open = "info" }
    }
    when (open) {
        "version" ->
            be.alwyapps.watch.ui.ChoiceDialog(
                "Version",
                pi.sources.map { s ->
                    s.id to listOfNotNull(videoLabel(s), s.size?.let { "%.1f GB".format(it.toDouble() / 1e9) }).joinToString(" · ")
                },
                src.id,
                onPick = {
                    be.alwyapps.watch.player.TrackPrefs
                        .setVersion(core, account, target.id, it)
                    version = it
                    open = null
                },
                onDismiss = { open = null },
            )
        "audio" ->
            be.alwyapps.watch.ui.ChoiceDialog(
                "Audio",
                audio.map { it.index.toString() to audioLabel(it) },
                currentAudio?.index?.toString(),
                onPick = { k ->
                    val np = picked(be.alwyapps.watch.core.TrackKind.AUDIO, audio.first { it.index.toString() == k })
                    be.alwyapps.watch.player.TrackPrefs
                        .set(core, account, key, np)
                    pref = np
                    open = null
                },
                onDismiss = { open = null },
            )
        "subs" ->
            be.alwyapps.watch.ui.ChoiceDialog(
                "Subtitles",
                listOf("auto" to "Automatic (settings)", "off" to "Off") + subs.map { it.index.toString() to subtitleLabel(it) },
                when {
                    p?.subs == null -> "auto"
                    p.subs == "off" -> "off"
                    else -> rememberedSubs?.index?.toString()
                },
                onPick = { k ->
                    val np =
                        when (k) {
                            "auto" ->
                                (
                                    pref ?: be.alwyapps.watch.core
                                        .TrackPref(null, null, null, null, false, null, null)
                                ).copy(subs = null, forced = false, subsTitle = null, subsNth = null)
                            "off" -> picked(be.alwyapps.watch.core.TrackKind.SUBTITLE, null)
                            else -> picked(be.alwyapps.watch.core.TrackKind.SUBTITLE, subs.first { it.index.toString() == k })
                        }
                    be.alwyapps.watch.player.TrackPrefs
                        .set(core, account, key, np)
                    pref = np
                    open = null
                },
                onDismiss = { open = null },
            )
        "info" -> FileInfoDialog(src) { open = null }
    }
}

@Composable
private fun SmallPill(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    onClick: () -> Unit,
) {
    val interaction =
        remember {
            androidx.compose.foundation.interaction
                .MutableInteractionSource()
        }
    val focused by interaction.collectIsFocusedAsState()
    Row(
        Modifier
            .clip(RoundedCornerShape(8.dp))
            .background(if (focused) Color.White else Color.Black.copy(alpha = 0.45f))
            .clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Icon(icon, null, Modifier.size(16.dp), tint = if (focused) Color.Black else OnSurface)
        Text(label, style = MaterialTheme.typography.labelMedium, color = if (focused) Color.Black else OnSurface, maxLines = 1)
    }
}

/** The file behind a title: name, container, size, bitrate and every stream. */
@Composable
private fun FileInfoDialog(
    src: be.alwyapps.watch.core.media.MediaSource,
    onDismiss: () -> Unit,
) {
    val lines =
        buildList {
            src.fileName?.let { add("File" to it) }
            src.container?.let { add("Container" to it.uppercase()) }
            src.size?.let { add("Size" to "%.2f GB".format(it.toDouble() / 1e9)) }
            src.bitrate?.let { add("Bitrate" to "%.1f Mb/s".format(it.toDouble() / 1e6)) }
            src.durationMs?.let {
                add(
                    "Duration" to
                        be.alwyapps.watch.player
                            .formatTime(it.toLong()),
                )
            }
            for (st in src.streams) {
                when (st.kind) {
                    be.alwyapps.watch.core.media.StreamKind.VIDEO ->
                        add(
                            "Video" to
                                listOfNotNull(
                                    st.width?.let { w -> "$w×${st.height}" },
                                    st.codec.uppercase(),
                                    st.profile,
                                    st.bitDepth?.let { "$it-bit" },
                                    st.hdr?.name?.takeIf { it != "SDR" },
                                ).joinToString(" "),
                        )
                    be.alwyapps.watch.core.media.StreamKind.AUDIO -> add("Audio" to audioLabel(st))
                    be.alwyapps.watch.core.media.StreamKind.SUBTITLE ->
                        add("Subtitles" to subtitleLabel(st) + " (" + st.codec.uppercase() + if (st.isExternal) ", external)" else ")")
                    else -> {}
                }
            }
        }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = be.alwyapps.watch.ui.Background,
        title = { Text("File info") },
        text = {
            androidx.compose.foundation.lazy.LazyColumn(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                items(lines.size) { n ->
                    val (k, v) = lines[n]
                    Row {
                        Text(k, Modifier.width(96.dp), style = MaterialTheme.typography.bodySmall, color = Muted)
                        Text(v, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        },
        confirmButton = { ActionButton("Close", onDismiss, primary = true) },
    )
}

/** The show's theme song, quietly, while its page is open (setting; like Plex). */
@androidx.annotation.OptIn(androidx.media3.common.util.UnstableApi::class)
@Composable
private fun ThemeMusic(
    account: String,
    itemId: String,
    isShow: Boolean,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val enabled = isShow && remember { PlayerSettings(core, isTvDevice(ctx)).themeMusic }
    var request by remember(itemId) { mutableStateOf<be.alwyapps.watch.core.media.HttpRequest?>(null) }
    LaunchedEffect(itemId, enabled) {
        if (enabled) request = runCatching { core.themeSong(account, itemId) }.getOrNull()
    }
    val req = request ?: return
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current.lifecycle
    androidx.compose.runtime.DisposableEffect(req) {
        val http =
            androidx.media3.datasource.DefaultHttpDataSource
                .Factory()
                .setDefaultRequestProperties(req.headers.associate { it.name to it.value })
        val p =
            androidx.media3.exoplayer.ExoPlayer
                .Builder(ctx)
                .setMediaSourceFactory(
                    androidx.media3.exoplayer.source
                        .DefaultMediaSourceFactory(
                            androidx.media3.datasource.DefaultDataSource
                                .Factory(ctx, http),
                        ),
                ).build()
        p.volume = 0.25f
        p.setMediaItem(
            androidx.media3.common.MediaItem
                .fromUri(req.url),
        )
        p.prepare()
        p.playWhenReady = true
        // The app in the background (or the player on top): silence.
        val observer =
            androidx.lifecycle.LifecycleEventObserver { _, e ->
                when (e) {
                    androidx.lifecycle.Lifecycle.Event.ON_PAUSE -> p.pause()
                    else -> {}
                }
            }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            p.release()
        }
    }
}

private fun isTvDevice(ctx: android.content.Context) = ctx.packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_LEANBACK)

/**
 * The overview, a few lines with an ellipsis; cut, it opens in full with OK (or a tap), since
 * a TV has no other way to read the rest.
 */
@Composable
private fun Overview(
    title: String,
    text: String,
) {
    val d = LocalDimens.current
    var cut by remember(text) { mutableStateOf(false) }
    var full by remember { mutableStateOf(false) }
    val body = @Composable { m: Modifier ->
        Text(
            text,
            m,
            style = if (d.isTv) MaterialTheme.typography.bodyMedium else MaterialTheme.typography.bodyLarge,
            color = OnSurface.copy(alpha = 0.85f),
            maxLines = if (d.isTv) 3 else 8,
            overflow = TextOverflow.Ellipsis,
            onTextLayout = { cut = cut || it.hasVisualOverflow },
        )
    }
    if (!cut) {
        body(Modifier)
    } else {
        be.alwyapps.watch.ui
            .FocusCard(onClick = { full = true }, glow = false, focusScale = 1f) { body(Modifier.padding(6.dp)) }
    }
    if (full) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { full = false },
            title = { Text(title) },
            text = {
                // TV: the text itself takes the focus and ↑/↓ scroll it (nothing else in it can
                // be focused, so the remote could not read past the first screen).
                val scroll = androidx.compose.foundation.rememberScrollState()
                Text(
                    text,
                    Modifier
                        .focusOnAppear()
                        .dpadScroll(scroll, focusable = true)
                        .verticalScroll(scroll)
                        .padding(6.dp),
                    style = MaterialTheme.typography.bodyLarge,
                )
            },
            confirmButton = { ActionButton("Close", { full = false }) },
            containerColor = be.alwyapps.watch.ui.Background,
        )
    }
}

/** "Cast" row: round photos, name and character (like Plex's cast row). */
@Composable
private fun CastRow(cast: List<be.alwyapps.watch.core.media.Person>) {
    val d = LocalDimens.current
    val photo = if (d.isTv) 88.dp else 76.dp
    // Entering the row starts at its first person (else the one under the element above, the
    // 4th or so, and the TV's pivot scrolling moved the row to it).
    val first =
        remember {
            androidx.compose.ui.focus
                .FocusRequester()
        }
    Column(Modifier.padding(top = 16.dp)) {
        Text("Cast", Modifier.padding(horizontal = d.gutter), style = MaterialTheme.typography.titleMedium)
        androidx.compose.foundation.lazy.LazyRow(
            Modifier.focusRestorer(first),
            contentPadding =
                androidx.compose.foundation.layout
                    .PaddingValues(horizontal = d.gutter, vertical = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(if (d.isTv) 20.dp else 14.dp),
        ) {
            items(cast.size) { n ->
                val p = cast[n]
                // Focusable on TV so the row scrolls with the D-pad.
                be.alwyapps.watch.ui.FocusCard(
                    onClick = {},
                    glow = false,
                    modifier = Modifier.width(photo + 28.dp).then(if (n == 0) Modifier.focusRequester(first) else Modifier),
                ) {
                    // Padded inside the card: its rounded clip (and the focus outline) cut the
                    // last letters of a name that reached its corner ("Dwayne Johnsor").
                    Column(
                        horizontalAlignment = Alignment.CenterHorizontally,
                        modifier = Modifier.fillMaxWidth().padding(start = 6.dp, end = 6.dp, top = 4.dp, bottom = 8.dp),
                    ) {
                        be.alwyapps.watch.ui
                            .UrlImage(p.photoUrl, photo, Modifier.size(photo).clip(RoundedCornerShape(50)))
                        Text(
                            p.name,
                            Modifier.padding(top = 6.dp),
                            style = MaterialTheme.typography.bodySmall,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                        )
                        p.role?.let {
                            Text(
                                it,
                                style = MaterialTheme.typography.labelSmall,
                                color = Muted,
                                maxLines = 1,
                                overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                            )
                        }
                    }
                }
            }
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

/** Round button with a caption under it (the action row of phone detail pages, like Plex). */
@Composable
private fun CaptionAction(
    caption: String,
    onClick: () -> Unit,
    highlighted: Boolean = false,
    icon: @Composable () -> Unit,
) {
    Column(
        Modifier
            .width(84.dp)
            .clip(RoundedCornerShape(14.dp))
            .clickable(onClick = onClick)
            .padding(vertical = 6.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(
            Modifier
                .size(48.dp)
                .clip(RoundedCornerShape(50))
                .background(if (highlighted) Accent else be.alwyapps.watch.ui.SurfaceHigh),
            contentAlignment = Alignment.Center,
        ) { icon() }
        Text(
            caption,
            style = MaterialTheme.typography.bodySmall,
            color = Muted,
            maxLines = 2,
            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
        )
    }
}

/** What the download caption says: "Download", "Starting…", "Downloading 42%", "Downloaded · 0.5 GB". */
private fun downloadCaption(
    dl: be.alwyapps.watch.core.downloads.Download?,
    pending: Boolean,
): String =
    when {
        dl == null -> if (pending) "Starting…" else "Download"
        dl.state == DownloadState.COMPLETED -> "Downloaded · ${dl.progressLabel()}"
        dl.state == DownloadState.PAUSED -> "Paused · ${dl.progressLabel()}"
        dl.state == DownloadState.WAITING -> "Waiting for network"
        dl.state == DownloadState.FAILED -> "Download failed"
        dl.downloaded == 0uL -> "Starting…"
        else -> "Downloading ${dl.progressLabel()}"
    }

/**
 * Download action of a movie or an episode page. Movies ([toggle]): download, cancel while
 * downloading, remove (after asking) once downloaded. Episodes open the sheet instead.
 */
@Composable
private fun DownloadAction(
    item: Item,
    dl: be.alwyapps.watch.core.downloads.Download?,
    toggle: Boolean,
    onOpen: () -> Unit,
) {
    val pending = DownloadPending.contains(item.id) && dl == null
    val click = rememberDownloadToggle(item, dl)
    CaptionAction(downloadCaption(dl, pending), if (toggle) click else onOpen, highlighted = dl?.state == DownloadState.COMPLETED) {
        DownloadStateIcon(dl, pending)
    }
}

/** A show's download action: what is downloading or downloaded for it. */
@Composable
private fun SeriesDownloadAction(
    series: Item,
    mine: List<be.alwyapps.watch.core.downloads.Download>,
    onOpen: () -> Unit,
) {
    val active = mine.filter { it.state != DownloadState.COMPLETED }
    val done = mine.size - active.size
    val pending = DownloadPending.contains(series.id) && mine.isEmpty()
    val caption =
        when {
            pending -> "Starting…"
            active.isNotEmpty() -> "Downloading ${active.size}"
            done > 0 -> "$done downloaded"
            else -> "Download"
        }
    // The ring follows the whole batch.
    val total = mine.sumOf { (it.total ?: 0uL).toLong() }
    val got = mine.sumOf { it.downloaded.toLong() }
    CaptionAction(caption, onOpen, highlighted = active.isEmpty() && done > 0) {
        when {
            pending || (active.isNotEmpty() && got == 0L) -> DownloadStateIcon(null, pending = true)
            active.isNotEmpty() -> ProgressRing(if (total > 0) got.toFloat() / total else 0f)
            done > 0 -> Icon(Icons.Download, null, Modifier.size(22.dp), tint = Color.Black)
            else -> Icon(Icons.Download, null, Modifier.size(22.dp))
        }
    }
}

/** Download/cancel/remove for a movie or an episode row (asks before removing a finished one). */
@Composable
private fun rememberDownloadToggle(
    item: Item,
    dl: be.alwyapps.watch.core.downloads.Download?,
): () -> Unit {
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
            title = { Text(if (dl.state == DownloadState.COMPLETED) "Remove the download?" else "Cancel the download?") },
            text = { Text("${item.title} (${dl.progressLabel()}) will be deleted from this phone.") },
            confirmButton = {
                ActionButton("Remove", {
                    core.removeDownload(dl.id)
                    DownloadPending.changed()
                    confirm = false
                }, primary = true)
            },
            dismissButton = { ActionButton("Keep", { confirm = false }) },
            containerColor = be.alwyapps.watch.ui.Background,
        )
    }
    return {
        when {
            dl == null -> {
                if (android.os.Build.VERSION.SDK_INT >= 33) permission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                DownloadPending.add(item.id)
                ctx.services.scope.launch(kotlinx.coroutines.Dispatchers.Main) {
                    runCatching { core.downloadItem(account, item.id, null) }
                    DownloadPending.remove(item.id)
                    startDownloads(ctx)
                }
            }
            // Finished or not, deleting asks first.
            else -> confirm = true
        }
    }
}

/** Episode rows: the round download button alone. */
@Composable
private fun DownloadButton(
    item: Item,
    dl: be.alwyapps.watch.core.downloads.Download?,
) {
    val pending = DownloadPending.contains(item.id) && dl == null
    DownloadStateButton(dl, rememberDownloadToggle(item, dl), pending = pending)
}

@Composable
private fun SeasonPicker(
    list: List<Item>,
    selected: Item,
    onMark: (Item) -> Unit,
    markFocus: androidx.compose.ui.focus.FocusRequester,
    /** "More seasons" (Seerr): only when some season can be requested. */
    onMore: (() -> Unit)?,
    moreFocus: androidx.compose.ui.focus.FocusRequester,
    /** Set just before [moreFocus] is asked for the focus: entering the row then keeps it. */
    direct: BooleanArray,
    onSelected: (Item) -> Unit,
) {
    val d = LocalDimens.current
    // Entering the row (↓ from the buttons, ↑ from the mark button) lands on the season shown,
    // not on whichever chip is nearest geometrically (the last one, from the action row).
    val selectedChip =
        remember {
            androidx.compose.ui.focus
                .FocusRequester()
        }
    Column {
        Text("Episodes", Modifier.padding(horizontal = d.gutter, vertical = 8.dp), style = MaterialTheme.typography.titleMedium)
        LazyRow(
            Modifier
                .focusProperties {
                    onEnter = {
                        // A request for a chip of its own ("More seasons" after its dialog) is kept.
                        if (direct[0]) direct[0] = false else runCatching { selectedChip.requestFocus() }
                    }
                }.focusGroup(),
            contentPadding = PaddingValues(horizontal = d.gutter),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // Long press (hold OK on a remote): this season watched or unwatched.
            items(list, key = { it.id }) { s ->
                val isSelected = s.id == selected.id
                ActionButton(
                    s.title,
                    { onSelected(s) },
                    if (isSelected) Modifier.focusRequester(selectedChip) else Modifier,
                    selected = isSelected,
                    onLongClick = { onMark(s) },
                )
            }
            if (onMore != null) {
                item(key = "more") {
                    ActionButton("More seasons", onMore, Modifier.focusRequester(moreFocus), icon = Icons.Add)
                }
            }
        }
        // The same as the long press, visible and in words, for the season shown: its own line
        // under the seasons (a lone ✓ next to them read as one more season, or as "selected").
        Row(Modifier.padding(start = d.gutter, end = d.gutter, top = 10.dp)) {
            ActionButton(
                "Mark ${selected.title} watched or unwatched",
                { onMark(selected) },
                Modifier.focusRequester(markFocus),
                icon = Icons.Check,
            )
        }
    }
}

@Composable
private fun EpisodeRow(
    ep: Item,
    dl: be.alwyapps.watch.core.downloads.Download?,
    onMenu: () -> Unit,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    Box(Modifier.padding(horizontal = d.gutter, vertical = 5.dp)) {
        FocusCard(
            onClick = { play(ctx, account, ep, fromStart = false) },
            // Long press (hold OK on a remote): watched / unwatched, download…
            onLongClick = onMenu,
            glow = false,
            focusScale = 1.02f,
            modifier =
                Modifier.fillMaxWidth().onFocusChanged {
                    if (it.isFocused) core.prefetchPlaybackInfo(account, ep.id, DeviceProfiles.get(ctx))
                },
        ) {
            Row(Modifier.fillMaxWidth().padding(6.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.width(if (d.isTv) 176.dp else 150.dp).clip(RoundedCornerShape(d.radius - 4.dp))) {
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
                            dl?.let { if (it.state == DownloadState.COMPLETED) it.progressLabel() else "downloading ${it.progressLabel()}" },
                        ).joinToString(" · "),
                        style = MaterialTheme.typography.bodySmall,
                        color = Subtle,
                    )
                    ep.overview?.let {
                        Text(it, color = Muted, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall)
                    }
                }
                // Phones/tablets: download an episode straight from the list.
                if (!d.isTv) DownloadButton(ep, dl)
            }
        }
    }
}
