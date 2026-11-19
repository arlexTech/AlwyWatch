// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListState
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.LibraryKind
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.ErrorBanner
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Rail
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.SkeletonRail
import be.alwyapps.watch.ui.rememberSwr

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

@Composable
fun HomeScreen() {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val home =
        rememberSwr(
            account,
            cached = { core.cachedHome(account) },
            refresh = { force -> core.refreshHome(account, force) },
        )
    val open: (Item) -> Unit = { nav.push(Route.Detail(it.id)) }
    // "Fully drawn" = rails on screen (from cache or network), for startup benchmarks.
    androidx.activity.compose.ReportDrawnWhen { home.value != null || home.error != null }
    LazyColumn(Modifier.fillMaxSize(), state = rememberPersistentListState("home.$account")) {
        item(key = "top") {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = d.gutter, vertical = 12.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Text("AlwyWatch", Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                ActionButton("Search", { nav.push(Route.Search) })
                if (core.seerrActive()) ActionButton("Requests", { nav.push(Route.Requests) })
                // No downloads on TV.
                if (!d.isTv) ActionButton("Downloads", { nav.push(Route.Downloads) })
                ActionButton("Settings", { nav.push(Route.Settings) })
            }
        }
        home.error?.let { msg -> item(key = "error") { ErrorBanner(msg) { home.reload() } } }
        val h = home.value
        if (h == null) {
            items(3) { SkeletonRail() }
        } else {
            item(key = "libraries") {
                LazyRow(
                    contentPadding = PaddingValues(horizontal = d.gutter),
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    items(h.libraries.filter { it.kind != LibraryKind.OTHER }, key = { it.id }) { lib ->
                        ActionButton(lib.title, { nav.push(Route.Library(lib.id, lib.title)) })
                    }
                }
            }
            if (core.keepActive()) item(key = "leaving") { LeavingSoonRail() }
            item(key = "resume") { Rail("Continue watching", h.resume, open) }
            item(key = "next") { Rail("Next up", h.nextUp, open) }
            items(h.latest, key = { "latest-${it.library.id}" }) { rail ->
                Rail("Latest in ${rail.library.title}", rail.items, open)
            }
            if (h.resume.isEmpty() && h.nextUp.isEmpty() && h.latest.all { it.items.isEmpty() }) {
                item { Text("Nothing here yet.", Modifier.padding(d.gutter)) }
            }
        }
        item { Spacer(Modifier.height(48.dp)) }
    }
}
