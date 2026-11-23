// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalItemOwners
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.LocalServerBadges
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.PosterCard
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.tr

/** What the home's Continue watching row held when its See all was chosen: (account, item). */
object ContinueWatchingList {
    var items: List<Pair<String, Item>> = emptyList()
}

/** Everything in Continue watching, as a grid (the row's See all). */
@Composable
fun ContinueWatchingScreen() {
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val entries = ContinueWatchingList.items
    LazyVerticalGrid(
        if (d.isTv) GridCells.FixedSize(d.posterWidth) else GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) { PageTitle(tr("Continue watching")) }
        items(entries, key = { (a, i) -> "$a/${i.id}" }) { (a, i) ->
            CompositionLocalProvider(
                LocalItemOwners provides mapOf(i.id to a),
                LocalServerBadges provides (entries.map { it.first }.distinct().size > 1),
            ) {
                PosterCard(i, d.posterWidth, onClick = { nav.push(Route.Detail(i.id, account = a)) }, showTitle = true, inContinueWatching = true)
            }
        }
    }
}
