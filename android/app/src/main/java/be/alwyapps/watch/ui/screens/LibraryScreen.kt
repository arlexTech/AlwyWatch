// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
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
import be.alwyapps.watch.core.media.ItemQuery
import be.alwyapps.watch.core.media.SortBy
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.ErrorBanner
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PosterCard
import be.alwyapps.watch.ui.Route
import kotlinx.coroutines.flow.distinctUntilChanged

private const val PAGE = 60

/** Pages of a library, each loaded cache-first; the next page is fetched well before the end. */
private class Pager(
    private val core: AlwyWatch,
    private val account: String,
    private val libraryId: String,
    private val sort: SortBy,
    private val descending: Boolean,
) {
    val items = mutableStateListOf<Item>()
    var total by mutableIntStateOf(-1)
    var error by mutableStateOf<String?>(null)
    private var loading = false

    private fun query(start: Int) = ItemQuery(libraryId, start.toUInt(), PAGE.toUInt(), sort, descending, unplayedOnly = false, search = null)

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

@Composable
fun LibraryScreen(
    id: String,
    title: String,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    var sort by remember { mutableStateOf(SortBy.TITLE) }
    val pager = remember(id, sort) { Pager(core, account, id, sort, descending = sort != SortBy.TITLE).apply { loadCached() } }
    val key = "lib.$account.$id.$sort"
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
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(title, Modifier.weight(1f).padding(top = 8.dp), style = MaterialTheme.typography.titleLarge)
                for ((label, s) in listOf("A–Z" to SortBy.TITLE, "Added" to SortBy.DATE_ADDED, "Released" to SortBy.RELEASE_DATE)) {
                    ActionButton(label, { sort = s }, primary = sort == s)
                }
            }
        }
        pager.error?.let { msg -> item(span = { GridItemSpan(maxLineSpan) }, key = "err") { ErrorBanner(msg) { pager.error = null } } }
        itemsIndexed(pager.items, key = { _, it -> it.id }) { _, item ->
            PosterCard(item, d.posterWidth, onClick = { nav.push(Route.Detail(item.id)) })
        }
        if (pager.total == 0) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("This library is empty.", color = Muted) }
        }
    }
}
