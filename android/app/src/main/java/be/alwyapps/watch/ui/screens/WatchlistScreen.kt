// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.ui.ItemsChanged
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalItemOwners
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.LocalServerBadges
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.PosterCard
import be.alwyapps.watch.ui.Route

/**
 * "My list", like Plex's watchlist: what the user saved to watch later, from every server of the
 * home (Plex's plex.tv watchlist, Jellyfin favorites). Cached first, then refreshed.
 */
@Composable
fun WatchlistScreen() {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val accounts =
        remember(account) {
            val all = runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id }
            val together = all.size > 1 && runCatching { core.getSetting(COMBINED_KEY) }.getOrNull() != "0"
            if (together) preferredFirst(core, all) else listOf(account)
        }

    fun combine(lists: List<Pair<String, List<Item>>>): List<Pair<String, Item>> =
        lists
            .flatMap { (a, items) -> items.map { a to it } }
            .distinctBy { (_, i) -> i.sameAs() }

    var entries by remember(accounts) {
        mutableStateOf(combine(accounts.map { a -> a to (core.cachedWatchlist(a)?.items ?: emptyList()) }))
    }
    var loaded by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val changed = ItemsChanged.version
    LaunchedEffect(accounts, changed) {
        val lists = mutableListOf<Pair<String, List<Item>>>()
        var failed: String? = null
        for (a in accounts) {
            try {
                lists += a to core.refreshWatchlist(a, changed > 0).items
            } catch (e: CoreException) {
                failed = e.message
                lists += a to (core.cachedWatchlist(a)?.items ?: emptyList())
            }
        }
        entries = combine(lists)
        error = failed.takeIf { entries.isEmpty() }
        loaded = true
    }
    LazyVerticalGrid(
        GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) { PageTitle("My list") }
        error?.let { item(span = { GridItemSpan(maxLineSpan) }) { Text(it, color = Muted) } }
        if (loaded && entries.isEmpty() && error == null) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "Nothing saved yet. On a movie or show, press the bookmark to keep it here.",
                    color = Muted,
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        items(entries, key = { (a, i) -> "$a/${i.id}" }) { (a, i) ->
            CompositionLocalProvider(
                LocalItemOwners provides mapOf(i.id to a),
                LocalServerBadges provides (entries.map { it.first }.distinct().size > 1),
            ) {
                PosterCard(i, d.posterWidth, onClick = { nav.push(Route.Detail(i.id, account = a)) }, showTitle = true)
            }
        }
    }
}
