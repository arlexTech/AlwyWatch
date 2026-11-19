// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.animation.Crossfade
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
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
    (h.resume + h.nextUp + h.latest.flatMap { it.items.take(4) })
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
    val home =
        rememberSwr(
            account,
            cached = { core.cachedHome(account) },
            refresh = { force -> core.refreshHome(account, force) },
        )
    // "Fully drawn" = rails on screen (from cache or network), for startup benchmarks.
    androidx.activity.compose.ReportDrawnWhen { home.value != null || home.error != null }
    if (d.isTv) TvHome(home.value, home.error) { home.reload() } else PhoneHome(home.value, home.error) { home.reload() }
}

@Composable
private fun Rails(
    h: Home,
    onFocus: ((Item) -> Unit)?,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val open: (Item) -> Unit = { nav.push(Route.Detail(it.id)) }
    val first =
        when {
            h.resume.isNotEmpty() -> "resume"
            h.nextUp.isNotEmpty() -> "next"
            else ->
                h.latest
                    .firstOrNull { it.items.isNotEmpty() }
                    ?.library
                    ?.id
        }
    Rail("Continue watching", h.resume, open, onFocus, landscape = true, focusKey = "home", defaultFocus = first == "resume")
    Rail("Next up", h.nextUp, open, onFocus, landscape = true, focusKey = "home", defaultFocus = first == "next")
    // After what the user is watching, before the "new" rails.
    if (core.keepActive()) LeavingSoonRail()
    for (rail in h.latest) {
        Rail("New in ${rail.library.title}", rail.items, open, onFocus, focusKey = "home", defaultFocus = first == rail.library.id)
    }
}

// ----- Phone / tablet -------------------------------------------------------------------------

@Composable
private fun PhoneHome(
    h: Home?,
    error: String?,
    reload: () -> Unit,
) {
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val featured = remember(h) { h?.let(::featured).orEmpty() }
    var glow by remember { mutableStateOf<Color?>(null) }
    LazyColumn(
        Modifier.fillMaxSize().ambientGlow(glow),
        state = rememberPersistentListState("home.$account"),
    ) {
        item(key = "hero") {
            if (featured.isEmpty()) {
                Spacer(Modifier.statusBarsPadding().height(16.dp))
            } else {
                PhoneHero(featured) { glow = it }
            }
        }
        error?.let { msg -> item(key = "error") { ErrorBanner(msg, reload) } }
        if (h == null) {
            items(3) { SkeletonRail() }
        } else {
            item(key = "libraries") {
                LazyRow(
                    contentPadding = PaddingValues(horizontal = d.gutter),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    items(h.libraries.filter { it.kind != LibraryKind.OTHER }, key = { it.id }) { lib ->
                        ActionButton(
                            lib.title,
                            { nav.push(Route.Library(lib.id, lib.title)) },
                            icon = if (lib.kind == LibraryKind.SHOWS) Icons.Tv else Icons.Movie,
                        )
                    }
                }
            }
            item(key = "rails") { Column { Rails(h, null) } }
            if (h.resume.isEmpty() && h.nextUp.isEmpty() && h.latest.all { it.items.isEmpty() }) {
                item { Text("Nothing here yet.", Modifier.padding(d.gutter), color = Muted) }
            }
        }
        item { Spacer(Modifier.height(32.dp)) }
    }
}

@Composable
private fun PhoneHero(
    featured: List<Item>,
    onColor: (Color?) -> Unit,
) {
    val ctx = LocalContext.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val pager = rememberPagerState { featured.size }
    val current = featured.getOrNull(pager.currentPage)
    LaunchedEffect(current?.id) { onColor(current?.ambient()) }
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val width = maxWidth
        val height = (width * 1.18f).coerceAtMost(620.dp)
        HorizontalPager(pager, Modifier.fillMaxWidth().height(height)) { page ->
            val item = featured[page]
            Box(Modifier.fillMaxSize()) {
                RemoteImage(item.id, item.backdrop(), width * 2, Modifier.fillMaxSize().fadeEdges(bottomFrom = 0.45f), background = Color.Transparent)
                Box(Modifier.fillMaxSize().background(Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.45f), 0.22f to Color.Transparent)))
                Column(
                    Modifier.align(Alignment.BottomStart).padding(start = 20.dp, end = 20.dp, bottom = 28.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    TitleArt(item, width * 0.7f)
                    HeroMeta(item)
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        if (item.kind == ItemKind.MOVIE || item.kind == ItemKind.EPISODE) {
                            ActionButton(
                                if (item.user.positionMs > 0u) "Resume" else "Play",
                                { play(ctx, account, item, fromStart = false) },
                                primary = true,
                                icon = Icons.Play,
                            )
                        }
                        ActionButton("Details", { nav.push(Route.Detail(item.detailId())) }, icon = Icons.Info)
                    }
                }
            }
        }
        Text(
            "AlwyWatch",
            Modifier.align(Alignment.TopStart).statusBarsPadding().padding(horizontal = 20.dp, vertical = 10.dp),
            style = MaterialTheme.typography.titleLarge,
        )
        Row(
            Modifier.align(Alignment.BottomEnd).padding(horizontal = 20.dp, vertical = 40.dp),
            horizontalArrangement = Arrangement.spacedBy(5.dp),
        ) {
            repeat(featured.size) { i ->
                Box(
                    Modifier
                        .height(6.dp)
                        .width(if (i == pager.currentPage) 18.dp else 6.dp)
                        .clip(RoundedCornerShape(50))
                        .background(if (i == pager.currentPage) Color.White else Color.White.copy(alpha = 0.35f)),
                )
            }
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
        val all = h?.let { it.resume + it.nextUp + it.latest.flatMap { r -> r.items } }.orEmpty()
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
                        item(key = "rails") { Column { Rails(h) { hero = it } } }
                    }
                    item { Spacer(Modifier.height(400.dp)) }
                }
            }
        }
    }
}
