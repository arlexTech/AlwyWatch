// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.Home
import be.alwyapps.watch.core.downloads.Download
import be.alwyapps.watch.core.downloads.DownloadState
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind
import be.alwyapps.watch.core.media.LibraryKind
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.ErrorBanner
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Rail
import be.alwyapps.watch.ui.RemoteImage
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.SkeletonRail
import be.alwyapps.watch.ui.ambient
import be.alwyapps.watch.ui.ambientGlow
import be.alwyapps.watch.ui.backdrop
import be.alwyapps.watch.ui.fadeEdges
import be.alwyapps.watch.ui.logo
import be.alwyapps.watch.ui.rememberSwr
import be.alwyapps.watch.ui.subtitle
import kotlinx.coroutines.launch

/** Restores a list's scroll position across cold starts (stored in the core settings). */
@Composable
fun rememberPersistentListState(key: String): LazyListState {
    val core = LocalCore.current
    val saved = remember(key) { core.getSetting("ui.scroll.$key")?.split(',')?.mapNotNull { it.toIntOrNull() } }
    val state = rememberLazyListState(saved?.getOrNull(0) ?: 0, saved?.getOrNull(1) ?: 0)
    DisposableEffect(key) {
        onDispose {
            runCatching {
                core.setSetting(
                    "ui.scroll.$key",
                    "${state.firstVisibleItemIndex},${state.firstVisibleItemScrollOffset}",
                )
            }
        }
    }
    return state
}

/** Hero candidates: what the user is in the middle of, then what is new, with artwork. */
private fun featured(h: Home): List<Item> =
    (h.continueWatching + h.latest.flatMap { it.items.take(4) })
        .distinctBy { if (it.kind == ItemKind.EPISODE) it.seriesId ?: it.id else it.id }
        .filter { it.backdrop() != null }
        .take(8)

/** The page to open: an episode in the hero opens its show. */
private fun Item.detailId(): String = if (kind == ItemKind.EPISODE) seriesId ?: id else id

@Composable
fun HomeScreen() {
    val core = LocalCore.current
    val account = LocalAccount.current
    val d = LocalDimens.current
    // Several servers shown together (default) or only the current account (Settings).
    val accounts = remember { runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id } }
    val combined = accounts.size > 1 && runCatching { core.getSetting(COMBINED_KEY) }.getOrNull() != "0"
    val ids = if (combined) preferredFirst(core, accounts) else listOf(account)
    val homes =
        ids.map { a ->
            androidx.compose.runtime.key(a) {
                rememberSwr(
                    a,
                    cached = { core.cachedHome(a) },
                    refresh = { force -> core.refreshHome(a, force) },
                )
            }
        }
    val parts = ids.zip(homes).mapNotNull { (a, h) -> h.value?.let { a to it } }
    val merged = remember(parts) { if (parts.isEmpty()) null else mergeHomes(parts) }
    val error = homes.firstNotNullOfOrNull { it.error }
    val reload = { homes.forEach { it.reload() } }
    // Watched state changed from an item's menu: the rails follow.
    val changed = be.alwyapps.watch.ui.ItemsChanged.version
    LaunchedEffect(changed) { if (changed > 0) reload() }
    // "Fully drawn" = rails on screen (from cache or network), for startup benchmarks.
    androidx.activity.compose.ReportDrawnWhen { merged != null || error != null }
    androidx.compose.runtime.CompositionLocalProvider(LocalOwners provides (merged?.second ?: emptyMap())) {
        if (d.isTv) TvHome(merged?.first, error, reload) else PhoneHome(merged?.first, error, reload)
    }
}

/** Setting: "0" shows only the current account's server on the home; anything else, all. */
const val COMBINED_KEY = "ui.combined"

/** Setting: the account whose copy wins when a title is on several servers. */
const val PREFERRED_KEY = "ui.preferred_account"

private fun preferredFirst(
    core: be.alwyapps.watch.core.AlwyWatch,
    accounts: List<String>,
): List<String> {
    val pref = runCatching { core.getSetting(PREFERRED_KEY) }.getOrNull()
    return accounts.sortedBy { if (it == pref) 0 else 1 }
}

/** Which account each item (and library) of a combined home comes from. */
private val LocalOwners = be.alwyapps.watch.ui.LocalItemOwners

/**
 * The same title on two servers: same TMDB/IMDB/TVDB id; episodes by show, season and number.
 */
private fun Item.sameAs(): String =
    when (kind) {
        ItemKind.EPISODE -> "ep:${seriesTitle?.lowercase()}:$seasonNumber:$episodeNumber"
        else ->
            providerIds.tmdb?.let { "tmdb:$kind:$it" }
                ?: providerIds.imdb?.let { "imdb:$it" }
                ?: providerIds.tvdb?.let { "tvdb:$kind:$it" }
                ?: "title:$kind:${title.lowercase()}:$year"
    }

/**
 * Continue watching across servers: all by recency (the core dates every entry), one per show;
 * a title on several servers comes from the preferred one.
 */
private fun mergeByRecency(
    homes: List<Pair<String, Home>>,
    owners: MutableMap<String, String>,
): List<Item> {
    val preferredCopy = mutableMapOf<String, Pair<String, Item>>()
    for ((a, h) in homes) for (i in h.continueWatching) preferredCopy.putIfAbsent(i.sameAs(), a to i)
    val shows = mutableSetOf<String>()
    return homes
        .flatMap { (_, h) -> h.continueWatching }
        .sortedByDescending { it.user.lastPlayedAt ?: 0L }
        .filter { shows.add(if (it.kind == ItemKind.EPISODE) "show:${it.seriesTitle?.lowercase()}" else it.sameAs()) }
        .map { i ->
            val (owner, chosen) = preferredCopy.getValue(i.sameAs())
            owners[chosen.id] = owner
            chosen
        }
}

/**
 * Several homes as one: rails of the same kind merged (alternating servers, preferred first),
 * a title on several servers kept once (from the first, preferred, server). Returns the home
 * and the owner account of every item and library.
 */
private fun mergeHomes(homes: List<Pair<String, Home>>): Pair<Home, Map<String, String>> {
    val owners = mutableMapOf<String, String>()

    fun merge(lists: List<Pair<String, List<Item>>>): List<Item> {
        val seen = mutableSetOf<String>()
        val out = mutableListOf<Item>()
        val max = lists.maxOfOrNull { it.second.size } ?: 0
        // Every server's copies first, so the preferred one wins duplicates.
        val firstOwner = mutableMapOf<String, Pair<String, Item>>()
        for ((a, items) in lists) for (i in items) firstOwner.putIfAbsent(i.sameAs(), a to i)
        for (n in 0 until max) {
            for ((_, items) in lists) {
                val i = items.getOrNull(n) ?: continue
                val k = i.sameAs()
                if (!seen.add(k)) continue
                val (owner, chosen) = firstOwner.getValue(k)
                owners[chosen.id] = owner
                out += chosen
            }
        }
        return out
    }
    if (homes.size == 1) {
        val (a, h) = homes.single()
        (h.continueWatching + h.latest.flatMap { it.items }).forEach { owners[it.id] = a }
        h.libraries.forEach { owners[it.id] = a }
        return h to owners
    }
    val first = homes.first().second
    // One rail per library kind (Movies, Shows), titled after the preferred server's.
    val kinds = homes.flatMap { (_, h) -> h.latest.map { it.library.kind } }.distinct()
    val latest =
        kinds.flatMap { kind ->
            if (kind == LibraryKind.OTHER) {
                homes.flatMap { (a, h) -> h.latest.filter { it.library.kind == kind }.onEach { r -> r.items.forEach { owners[it.id] = a } } }
            } else {
                val rails = homes.flatMap { (a, h) -> h.latest.filter { it.library.kind == kind }.map { a to it } }
                val (libOwner, lead) = rails.first()
                owners[lead.library.id] = libOwner
                listOf(lead.copy(items = merge(rails.map { (a, r) -> a to r.items })))
            }
        }
    // Library buttons: the preferred server's, plus kinds only another server has.
    val libraries = mutableListOf<be.alwyapps.watch.core.media.MediaLibrary>()
    for ((a, h) in homes) {
        for (lib in h.libraries) {
            if (libraries.none { it.kind == lib.kind && lib.kind != LibraryKind.OTHER }) {
                libraries += lib
                owners[lib.id] = a
            }
        }
    }
    // One server's "continue watching" can be another's "next up": show it once, in progress.
    return first.copy(
        continueWatching = mergeByRecency(homes, owners),
        libraries = libraries,
        latest = latest,
        fetchedAt = homes.minOf { it.second.fetchedAt },
    ) to owners
}

@Composable
private fun Rails(
    h: Home,
    onFocus: ((Item) -> Unit)?,
    offline: Boolean = false,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    // Offline, a show opens its downloaded episodes (its page needs the server).
    val owners = LocalOwners.current
    val open: (Item) -> Unit = {
        if (offline && it.kind == ItemKind.SERIES) {
            nav.push(Route.DownloadedShow(it.id, it.title))
        } else {
            nav.push(Route.Detail(it.id, account = owners[it.id]))
        }
    }
    val first =
        when {
            h.continueWatching.isNotEmpty() -> "resume"
            else ->
                h.latest
                    .firstOrNull { it.items.isNotEmpty() }
                    ?.library
                    ?.id
        }
    // TV: 16:9 cards under the hero. Phones: posters with progress, like Plex.
    val landscape = d.isTv
    // Like Plex: in progress and next episodes together, most recently watched first.
    Rail("Continue watching", h.continueWatching, open, onFocus, landscape = landscape, focusKey = "home", defaultFocus = first == "resume")
    // After what the user is watching, before the "new" rails.
    if (core.keepActive() && !offline) LeavingSoonRail()
    val account = LocalAccount.current
    val servers = remember(account) { runCatching { core.accounts() }.getOrDefault(emptyList()).associate { it.id to it.serverName } }
    for (rail in h.latest) {
        Rail(
            if (d.isTv) "New in ${rail.library.title}" else "Recently added in ${rail.library.title}",
            rail.items,
            open,
            onFocus,
            focusKey = "home",
            defaultFocus = first == rail.library.id,
            // The servers the rail comes from ("HomePlex · HomeJelly" when combined).
            subtitle =
                (
                    rail.items
                        .mapNotNull { owners[it.id] }
                        .distinct()
                        .ifEmpty { listOf(account) }
                ).mapNotNull { servers[it] }
                    .joinToString(" · ")
                    .takeIf { !d.isTv && it.isNotEmpty() },
            onSeeAll = if (offline) null else ({ nav.push(Route.Library(rail.library.id, rail.library.title, owners[rail.library.id])) }),
        )
    }
}

// ----- Phone / tablet -------------------------------------------------------------------------

@Composable
private fun PhoneHome(
    h: Home?,
    error: String?,
    reload: () -> Unit,
) {
    val owners = LocalOwners.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val core = LocalCore.current
    var reachable by remember(account) { mutableStateOf<Boolean?>(null) }
    // Downloads mode: the same home, made of what is on this phone (the top bar toggles it).
    var offline by remember { mutableStateOf(runCatching { core.getSetting("ui.downloads_mode") }.getOrNull() == "1") }
    val setOffline = { on: Boolean ->
        offline = on
        runCatching { core.setSetting("ui.downloads_mode", if (on) "1" else "0") }
    }
    Column(Modifier.fillMaxSize()) {
        HomeTopBar(offline) { setOffline(!offline) }
        val (downloads, _) = rememberDownloads()
        val completed = downloads.filter { it.accountId == account && it.state == DownloadState.COMPLETED }
        val active = downloads.count { it.accountId == account && it.state != DownloadState.COMPLETED }
        val shown =
            if (offline && h != null) {
                remember(h, completed.map { it.itemId }) { offlineHome(h, completed) { id -> core.cachedItem(account, id)?.item } }
            } else {
                h
            }
        LazyColumn(
            Modifier.fillMaxSize(),
            state = rememberPersistentListState("home.$account"),
        ) {
            if (offline && active > 0) {
                item(key = "active") {
                    ActionButton(
                        "Downloading $active · manage",
                        { nav.push(Route.ManageDownloads) },
                        Modifier.padding(horizontal = d.gutter, vertical = 8.dp),
                        icon = Icons.Download,
                    )
                }
            }
            if (!offline) {
                item(key = "status") {
                    ServerStatus(onDownloads = { setOffline(true) }, onReachable = {
                        // Back online after a failure: refresh what the cache showed.
                        if (reachable == false) reload()
                        reachable = it
                    })
                }
            }
            // The status card already says what is wrong with the server.
            if (reachable != false && !offline) error?.let { msg -> item(key = "error") { ErrorBanner(msg, reload) } }
            val h = shown
            if (h == null) {
                if (reachable != false && !offline) items(3) { SkeletonRail() }
            } else {
                if (!offline) {
                    item(key = "libraries") {
                        LazyRow(
                            contentPadding = PaddingValues(start = d.gutter, end = d.gutter, top = 4.dp),
                            horizontalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            items(h.libraries.filter { it.kind != LibraryKind.OTHER }, key = { it.id }) { lib ->
                                ActionButton(
                                    lib.title,
                                    { nav.push(Route.Library(lib.id, lib.title, owners[lib.id])) },
                                    icon = if (lib.kind == LibraryKind.SHOWS) Icons.Tv else Icons.Movie,
                                )
                            }
                        }
                    }
                }
                item(key = "rails") { Column { Rails(h, null, offline) } }
                if (h.continueWatching.isEmpty() && h.latest.all { it.items.isEmpty() }) {
                    item {
                        Text(
                            if (offline) "Nothing downloaded yet. Download from a movie or show page." else "Nothing here yet.",
                            Modifier.padding(d.gutter),
                            color = Muted,
                        )
                    }
                }
            }
            item { Spacer(Modifier.height(32.dp)) }
        }
    }
}

/**
 * The same home with only what is downloaded: in-progress and next episodes that are on the
 * phone, downloaded movies and shows in their library's rail. Rails left empty disappear.
 */
private fun offlineHome(
    h: Home,
    done: List<Download>,
    cached: (String) -> Item?,
): Home {
    val ids = done.map { it.itemId }.toSet()
    val items = done.mapNotNull { cached(it.itemId) }
    val movies = items.filter { it.kind == ItemKind.MOVIE }
    // Shows: from the download's metadata, or (older downloads) its cached episode.
    val seriesIds = (done.mapNotNull { it.episode?.seriesId } + items.mapNotNull { it.seriesId }).distinct()
    val shows =
        seriesIds.mapNotNull { sid ->
            cached(sid) ?: items.firstOrNull { it.seriesId == sid }?.showFor()
        }
    // One library per kind gets them (downloads don't know their library).
    val used = mutableSetOf<LibraryKind>()
    val latest =
        h.latest.map { rail ->
            val kind = rail.library.kind
            val items =
                when {
                    !used.add(kind) -> emptyList()
                    kind == LibraryKind.MOVIES -> movies
                    kind == LibraryKind.SHOWS -> shows
                    else -> emptyList()
                }
            rail.copy(items = items)
        }
    return h.copy(continueWatching = h.continueWatching.filter { it.id in ids }, latest = latest)
}

/** Plex-style bar: the name on the left; search, downloads and you (settings) on the right. */
@Composable
private fun HomeTopBar(
    offline: Boolean,
    onToggleDownloads: () -> Unit,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val me = remember(account) { runCatching { core.accounts() }.getOrNull()?.firstOrNull { it.id == account } }
    Row(
        Modifier
            .fillMaxWidth()
            .statusBarsPadding()
            .padding(start = d.gutter, end = d.gutter - 6.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            androidx.compose.ui.text.buildAnnotatedString {
                append("alwy")
                pushStyle(
                    androidx.compose.ui.text
                        .SpanStyle(color = be.alwyapps.watch.ui.Accent),
                )
                append("watch")
                pop()
            },
            Modifier.weight(1f),
            style =
                MaterialTheme.typography.headlineMedium.copy(
                    fontWeight =
                        androidx.compose.ui.text.font
                            .FontWeight(800),
                ),
        )
        TopIcon(Icons.Search, "Search") { nav.push(Route.Search) }
        TopIcon(Icons.Download, if (offline) "Leave downloads" else "Downloads", active = offline, onClick = onToggleDownloads)
        // You: settings, accounts and servers.
        Box(
            Modifier
                .padding(start = 6.dp)
                .size(38.dp)
                .clip(androidx.compose.foundation.shape.CircleShape)
                .background(Brush.linearGradient(listOf(be.alwyapps.watch.ui.Accent, Color(0xFFFF6A3D))))
                .clickable { nav.push(Route.Settings) },
            contentAlignment = Alignment.Center,
        ) {
            Text(
                me?.userName?.firstOrNull()?.uppercase() ?: "?",
                color = Color.Black,
                style = MaterialTheme.typography.titleMedium,
            )
        }
    }
}

@Composable
private fun TopIcon(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    label: String,
    active: Boolean = false,
    onClick: () -> Unit,
) {
    Box(
        Modifier
            .size(44.dp)
            .clip(androidx.compose.foundation.shape.CircleShape)
            .background(if (active) be.alwyapps.watch.ui.Accent else Color.Transparent)
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { androidx.compose.material3.Icon(icon, label, Modifier.size(24.dp), tint = if (active) Color.Black else be.alwyapps.watch.ui.OnSurface) }
}

private sealed interface Reach {
    data object Checking : Reach

    data object Ok : Reach

    data class Failed(
        val message: String,
    ) : Reach
}

/**
 * While the server does not answer: "Looking for <server>…", then after 10 s why it can't be
 * reached, with Retry. A way to the downloads is there the whole time (no waiting to watch
 * something offline). Checks again when a network comes back.
 */
@Composable
private fun ServerStatus(
    onDownloads: () -> Unit,
    onReachable: (Boolean) -> Unit,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val server = remember(account) { runCatching { core.accounts() }.getOrNull()?.firstOrNull { it.id == account }?.serverName ?: "your server" }
    var state by remember(account) { mutableStateOf<Reach>(Reach.Checking) }
    var attempt by remember { androidx.compose.runtime.mutableIntStateOf(0) }
    // Short checks stay invisible: the card shows only after a moment without an answer.
    var slow by remember { mutableStateOf(false) }
    val cm = remember { ctx.getSystemService(android.net.ConnectivityManager::class.java) }
    DisposableEffect(cm) {
        val cb =
            object : android.net.ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: android.net.Network) {
                    if (state is Reach.Failed) attempt++
                }
            }
        runCatching { cm?.registerDefaultNetworkCallback(cb) }
        onDispose { runCatching { cm?.unregisterNetworkCallback(cb) } }
    }
    LaunchedEffect(account, attempt) {
        state = Reach.Checking
        slow = false
        val online =
            cm?.getNetworkCapabilities(cm.activeNetwork)?.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
        if (!online) {
            state = Reach.Failed("You're offline.")
            onReachable(false)
            return@LaunchedEffect
        }
        val reveal =
            launch {
                kotlinx.coroutines.delay(1200)
                slow = true
            }
        state =
            try {
                core.ping(account)
                Reach.Ok
            } catch (e: be.alwyapps.watch.core.CoreException) {
                Reach.Failed(e.message ?: "Unknown error")
            }
        reveal.cancel()
        onReachable(state == Reach.Ok)
    }
    val s = state
    if (s == Reach.Ok || (s == Reach.Checking && !slow)) return
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = d.gutter, vertical = 8.dp)
            .clip(RoundedCornerShape(d.radius + 4.dp))
            .background(be.alwyapps.watch.ui.SurfaceColor)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            if (s is Reach.Failed) {
                Box(
                    Modifier
                        .size(10.dp)
                        .clip(androidx.compose.foundation.shape.CircleShape)
                        .background(be.alwyapps.watch.ui.Danger),
                )
            } else {
                androidx.compose.material3.CircularProgressIndicator(Modifier.size(20.dp), color = be.alwyapps.watch.ui.Accent, strokeWidth = 2.5.dp)
            }
            Column(Modifier.weight(1f)) {
                Text(
                    if (s is Reach.Failed) "Can't reach $server" else "Looking for $server…",
                    style = MaterialTheme.typography.titleSmall,
                )
                Text(
                    if (s is Reach.Failed) s.message else "Your downloads are ready if you don't want to wait.",
                    style = MaterialTheme.typography.bodySmall,
                    color = Muted,
                    maxLines = 3,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            ActionButton("Go to downloads", onDownloads, primary = true, icon = Icons.Download)
            if (s is Reach.Failed) ActionButton("Retry", { attempt++ })
        }
    }
}

/** The title's logo art when the server has one, otherwise the title in big type. */
@Composable
fun TitleArt(
    item: Item,
    maxWidth: Dp,
    big: Boolean = false,
) {
    val title = if (item.kind == ItemKind.EPISODE) item.seriesTitle ?: item.title else item.title
    val logo = item.logo()
    if (logo != null) {
        RemoteImage(
            item.detailId(),
            logo,
            maxWidth,
            Modifier.widthIn(max = maxWidth).heightIn(max = if (big) 130.dp else 90.dp).fillMaxWidth(),
            contentScale = ContentScale.Fit,
            contentDescription = title,
            background = Color.Transparent,
            alignment = Alignment.CenterStart,
        )
    } else {
        Text(
            title,
            Modifier.widthIn(max = maxWidth),
            style = if (big) MaterialTheme.typography.displayMedium else MaterialTheme.typography.displaySmall,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
fun HeroMeta(item: Item) {
    val parts =
        listOfNotNull(
            if (item.kind == ItemKind.EPISODE) {
                listOfNotNull(
                    if (item.seasonNumber != null && item.episodeNumber != null) "S${item.seasonNumber} · E${item.episodeNumber}" else null,
                    item.runtimeMs?.let {
                        be.alwyapps.watch.ui
                            .formatRuntime(it.toLong())
                    },
                ).joinToString(" · ").ifEmpty { null }
            } else {
                item.subtitle().ifEmpty { null }
            },
            item.officialRating,
            item.communityRating?.let { "★ %.1f".format(it) },
            item.genres
                .take(2)
                .joinToString(", ")
                .ifEmpty { null },
        )
    Text(parts.joinToString("  ·  "), style = MaterialTheme.typography.bodyMedium, color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
}

// ----- TV -------------------------------------------------------------------------------------

@Composable
private fun TvHome(
    h: Home?,
    error: String?,
    reload: () -> Unit,
) {
    val d = LocalDimens.current
    val featured = remember(h) { h?.let(::featured).orEmpty() }
    var hero by remember(featured) {
        val remembered = be.alwyapps.watch.ui.FocusMemory["home"]
        val all = h?.let { it.continueWatching + it.latest.flatMap { r -> r.items } }.orEmpty()
        mutableStateOf(all.firstOrNull { remembered?.endsWith("|${it.id}") == true && it.backdrop() != null } ?: featured.firstOrNull())
    }
    BoxWithConstraints(Modifier.fillMaxSize().ambientGlow(hero?.ambient())) {
        val screenH = maxHeight
        // Artwork on the right two thirds, fading into the background on the left and bottom.
        Crossfade(hero, Modifier.align(Alignment.TopEnd).fillMaxWidth().height(screenH * 0.78f), animationSpec = tween(350), label = "hero") { item ->
            if (item != null) {
                // Faded to transparent (not darkened) so it melts into the ambient glow.
                RemoteImage(
                    item.id,
                    item.backdrop(),
                    1280.dp,
                    Modifier.fillMaxSize().fadeEdges(bottomFrom = 0.4f, leftTo = 0.6f),
                    background = Color.Transparent,
                )
            }
        }
        Column(Modifier.fillMaxSize()) {
            Box(Modifier.fillMaxWidth().height(screenH * 0.46f).padding(start = d.gutter, top = 36.dp, end = d.gutter)) {
                Crossfade(hero, animationSpec = tween(250), label = "heroText") { item ->
                    if (item != null) {
                        Column(Modifier.fillMaxWidth(0.48f), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                            TitleArt(item, 400.dp, big = true)
                            HeroMeta(item)
                            item.overview?.let {
                                Text(it, style = MaterialTheme.typography.bodyMedium, color = Muted, maxLines = 3, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
            }
            // Focused row pinned just below the hero (room for its title).
            be.alwyapps.watch.ui.TvScrollSpec(leading = 58.dp) {
                LazyColumn(Modifier.fillMaxWidth().weight(1f)) {
                    error?.let { msg -> item(key = "error") { ErrorBanner(msg, reload) } }
                    if (h == null) {
                        items(2) { SkeletonRail() }
                    } else {
                        item(key = "rails") { Column { Rails(h, { hero = it }) } }
                    }
                    item { Spacer(Modifier.height(400.dp)) }
                }
            }
        }
    }
}
