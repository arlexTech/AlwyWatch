// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind
import be.alwyapps.watch.core.media.ItemQuery
import be.alwyapps.watch.core.media.SortBy
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.ErrorBanner
import be.alwyapps.watch.ui.FocusMemory
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PosterCard
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.initialFocus
import kotlinx.coroutines.flow.distinctUntilChanged

private const val PAGE = 60

/** Pages of a library, each loaded cache-first; the next page is fetched well before the end. */
private class Pager(
    private val core: AlwyWatch,
    private val account: String,
    private val libraryId: String,
    private val sort: SortBy,
    private val descending: Boolean,
    private val genreId: String? = null,
    private val collectionId: String? = null,
) {
    val items = mutableStateListOf<Item>()
    var total by mutableIntStateOf(-1)
    var error by mutableStateOf<String?>(null)
    private var loading = false

    private fun query(start: Int) =
        ItemQuery(
            libraryId,
            start.toUInt(),
            PAGE.toUInt(),
            sort,
            descending,
            unplayedOnly = false,
            search = null,
            genreId = genreId,
            collectionId = collectionId,
        )

    /** Synchronously fills every page we already have on disk, so the grid appears complete. */
    fun loadCached() {
        var start = 0
        while (true) {
            val page = runCatching { core.cachedItems(account, query(start)) }.getOrNull()?.page ?: break
            items.addAll(page.items)
            total = page.total.toInt()
            start += PAGE
            if (page.items.size < PAGE) break
        }
    }

    suspend fun refreshFirst() = load(0, replace = true)

    suspend fun loadMoreIfNeeded(lastVisible: Int) {
        if (loading || error != null) return
        if (total >= 0 && items.size >= total) return
        if (lastVisible >= items.size - PAGE / 2) load(items.size, replace = false)
    }

    private suspend fun load(
        start: Int,
        replace: Boolean,
    ) {
        loading = true
        try {
            val page = core.refreshItems(account, query(start), false).page
            if (replace) {
                // Keep cached pages beyond the first; they revalidate as the user scrolls.
                for (i in page.items.indices) if (i < items.size) items[i] = page.items[i] else items.add(page.items[i])
            } else {
                items.addAll(page.items)
            }
            total = page.total.toInt()
            error = null
        } catch (e: CoreException) {
            error = e.message
        } finally {
            loading = false
        }
    }
}

private enum class LibTab(
    val label: String,
) {
    RECOMMENDED("Recommended"),
    BROWSE("Browse"),
    COLLECTIONS("Collections"),
    CATEGORIES("Categories"),
}

/**
 * A library like Plex: Recommended (the home rails, only this library's kind), Browse (the
 * whole library, sortable), Collections and Categories (genres). The tab is remembered.
 */
@Composable
fun LibraryScreen(
    id: String,
    title: String,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val d = LocalDimens.current
    val tabKey = "ui.libtab.$account.$id"
    var tab by remember(id) {
        mutableStateOf(runCatching { LibTab.valueOf(core.getSetting(tabKey) ?: "") }.getOrDefault(LibTab.RECOMMENDED))
    }
    Column(Modifier.fillMaxSize()) {
        Column(Modifier.padding(start = d.gutter, end = d.gutter, top = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            be.alwyapps.watch.ui
                .PageTitle(title)
            androidx.compose.foundation.lazy.LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                items(LibTab.entries.size) { i ->
                    val t = LibTab.entries[i]
                    ActionButton(t.label, {
                        tab = t
                        runCatching { core.setSetting(tabKey, t.name) }
                    }, if (i == 0) Modifier.initialFocus() else Modifier, primary = tab == t)
                }
            }
        }
        when (tab) {
            LibTab.RECOMMENDED -> Recommended(id, title)
            LibTab.BROWSE -> LibraryGrid(id, title, null, null, header = false)
            LibTab.COLLECTIONS -> Collections(id)
            LibTab.CATEGORIES -> Categories(id, title)
        }
    }
}

/** The home's rails, keeping this library's kind only (movies, or shows and their episodes). */
@Composable
private fun Recommended(
    id: String,
    title: String,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val home =
        be.alwyapps.watch.ui.rememberSwr(
            account,
            cached = { core.cachedHome(account) },
            refresh = { force -> core.refreshHome(account, force) },
        )
    val h = home.value
    val kind = h?.libraries?.firstOrNull { it.id == id }?.kind
    val keep: (Item) -> Boolean = {
        when (kind) {
            be.alwyapps.watch.core.media.LibraryKind.MOVIES -> it.kind == ItemKind.MOVIE
            be.alwyapps.watch.core.media.LibraryKind.SHOWS -> it.kind == ItemKind.EPISODE || it.kind == ItemKind.SERIES
            else -> true
        }
    }
    val open: (Item) -> Unit = { nav.push(Route.Detail(it.id, account = account)) }
    androidx.compose.foundation.lazy.LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding =
            androidx.compose.foundation.layout
                .PaddingValues(bottom = 32.dp),
    ) {
        home.error?.let { msg -> item { ErrorBanner(msg) { home.reload() } } }
        if (h == null) {
            items(3) {
                be.alwyapps.watch.ui
                    .SkeletonRail()
            }
        } else {
            item {
                Column {
                    be.alwyapps.watch.ui
                        .Rail("Continue watching", h.continueWatching.filter(keep), open, focusKey = "lib.$id")
                    h.latest.firstOrNull { it.library.id == id }?.let { rail ->
                        be.alwyapps.watch.ui
                            .Rail("Recently added", rail.items, open, focusKey = "lib.$id")
                    }
                }
            }
        }
    }
}

/** Collections of a library, as posters; one opens its titles. */
@Composable
private fun Collections(id: String) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val list =
        be.alwyapps.watch.ui.rememberSwr(
            account,
            "collections",
            id,
            cached = { core.cachedCollections(account, id) },
            refresh = { core.refreshCollections(account, id, it) },
        )
    LazyVerticalGrid(
        GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        contentPadding =
            androidx.compose.foundation.layout
                .PaddingValues(d.gutter),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        list.error?.let { msg -> item(span = { GridItemSpan(maxLineSpan) }) { ErrorBanner(msg) { list.reload() } } }
        val cols = list.value
        if (cols != null && cols.isEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("No collections in this library.", color = Muted) }
        }
        itemsIndexed(cols.orEmpty(), key = { _, it -> it.id }) { _, c ->
            PosterCard(c, d.posterWidth, onClick = { nav.push(Route.Filtered(id, c.title, null, c.id, account)) }, showTitle = true)
        }
    }
}

/** Genres of a library; one opens the library narrowed to it. */
@Composable
private fun Categories(
    id: String,
    title: String,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val list =
        be.alwyapps.watch.ui.rememberSwr(
            account,
            "genres",
            id,
            cached = { core.cachedGenres(account, id) },
            refresh = { core.refreshGenres(account, id, it) },
        )
    androidx.compose.foundation.lazy.LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding =
            androidx.compose.foundation.layout
                .PaddingValues(horizontal = d.gutter, vertical = 16.dp),
    ) {
        list.error?.let { msg -> item { ErrorBanner(msg) { list.reload() } } }
        item {
            be.alwyapps.watch.ui.Group(null) {
                list.value.orEmpty().forEachIndexed { i, g ->
                    if (i > 0) {
                        be.alwyapps.watch.ui
                            .Divider()
                    }
                    be.alwyapps.watch.ui.ListRow(
                        g.name,
                        onClick = { nav.push(Route.Filtered(id, "${g.name} · $title", g.id, null, account)) },
                        trailing = {
                            be.alwyapps.watch.ui
                                .Chevron()
                        },
                    )
                }
            }
        }
    }
}

/**
 * A library's titles as a sortable poster grid (Browse), or narrowed to a genre or a
 * collection. [header]: its own title (a pushed page) or not (inside the library tabs).
 */
@Composable
fun LibraryGrid(
    id: String,
    title: String,
    genreId: String?,
    collectionId: String?,
    header: Boolean = true,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    // A collection keeps its own order (saga order); libraries and genres sort.
    var sort by remember { mutableStateOf(SortBy.TITLE) }
    val sortable = collectionId == null
    val pager =
        remember(id, sort, genreId, collectionId) {
            Pager(core, account, id, sort, descending = sort != SortBy.TITLE, genreId, collectionId).apply { loadCached() }
        }
    val key = "lib.$account.$id.$sort.${genreId.orEmpty()}.${collectionId.orEmpty()}"
    val saved = remember(key) { core.getSetting("ui.scroll.$key")?.toIntOrNull() ?: 0 }
    val grid = rememberLazyGridState(saved)
    DisposableEffect(key) { onDispose { runCatching { core.setSetting("ui.scroll.$key", grid.firstVisibleItemIndex.toString()) } } }

    LaunchedEffect(pager) { pager.refreshFirst() }
    LaunchedEffect(pager) {
        snapshotFlow {
            grid.layoutInfo.visibleItemsInfo
                .lastOrNull()
                ?.index ?: 0
        }.distinctUntilChanged()
            .collect { pager.loadMoreIfNeeded(it) }
    }

    LazyVerticalGrid(
        GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        state = grid,
        contentPadding =
            androidx.compose.foundation.layout
                .PaddingValues(d.gutter),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }, key = "header") {
            Column(Modifier.padding(bottom = 4.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                if (header) {
                    be.alwyapps.watch.ui
                        .PageTitle(title)
                }
                if (pager.total > 0) {
                    Text("%,d titles".format(pager.total), style = MaterialTheme.typography.bodyMedium, color = Muted)
                }
                if (sortable) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for ((label, s) in listOf("A–Z" to SortBy.TITLE, "Recently added" to SortBy.DATE_ADDED, "Release date" to SortBy.RELEASE_DATE)) {
                            ActionButton(label, { sort = s }, primary = sort == s)
                        }
                    }
                }
            }
        }
        pager.error?.let { msg -> item(span = { GridItemSpan(maxLineSpan) }, key = "err") { ErrorBanner(msg) { pager.error = null } } }
        itemsIndexed(pager.items, key = { _, it -> it.id }) { i, item ->
            val memo = "lib.$id|${item.id}"
            val focusMe = FocusMemory["lib.$id"]?.let { it == memo } ?: (i == 0)
            PosterCard(
                item,
                d.posterWidth,
                onClick = { nav.push(Route.Detail(item.id, account = account)) },
                onFocus = { FocusMemory["lib.$id"] = memo },
                cardModifier = if (focusMe && d.isTv && header) Modifier.initialFocus() else Modifier,
                showTitle = true,
            )
        }
        if (pager.total == 0) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("Nothing here.", color = Muted) }
        }
    }
}
