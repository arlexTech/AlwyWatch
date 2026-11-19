// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.seerr.Availability
import be.alwyapps.watch.core.seerr.SeerrResult
import be.alwyapps.watch.ui.Badge
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.PosterCard
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.UrlImage
import be.alwyapps.watch.ui.dpadEscape
import kotlinx.coroutines.delay

/**
 * One search for everything: your libraries first, then (with Seerr) what you could request.
 * Search as you type (debounced); works with the TV on-screen keyboard and voice input.
 */
@Composable
fun SearchScreen() {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    // Survives opening a result and coming back (screens are disposed by the navigator).
    var query by rememberSaveable { mutableStateOf(SearchMemory.query) }
    LaunchedEffect(query) { SearchMemory.query = query }
    var results by remember { mutableStateOf<List<Item>>(emptyList()) }
    var requestable by remember { mutableStateOf<List<SeerrResult>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    val focus = remember { FocusRequester() }
    // Keyboard straight away only for a new search, not when coming back to results.
    LaunchedEffect(Unit) { if (query.isEmpty()) runCatching { focus.requestFocus() } }
    LaunchedEffect(query) {
        if (query.length < 2) {
            results = emptyList()
            requestable = emptyList()
            return@LaunchedEffect
        }
        delay(300)
        try {
            results = core.search(account, query, 60u)
            error = null
        } catch (e: CoreException) {
            error = e.message
        }
        if (core.seerrActive()) {
            requestable =
                runCatching { core.seerrSearch(query) }
                    .getOrDefault(emptyList())
                    .filter { it.availability != Availability.AVAILABLE }
                    .take(20)
        }
    }
    LazyVerticalGrid(
        GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                PageTitle("Search")
                Field(
                    query,
                    { query = it },
                    Modifier.fillMaxWidth().focusRequester(focus).dpadEscape(),
                    placeholder = { Text("Movies, shows, episodes…") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                    leadingIcon = Icons.Search,
                    pill = true,
                )
            }
        }
        error?.let { item(span = { GridItemSpan(maxLineSpan) }) { Text(it, color = Muted) } }
        if (results.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("In your library", style = MaterialTheme.typography.titleMedium) }
        }
        items(results, key = { it.id }) { PosterCard(it, d.posterWidth, onClick = { nav.push(Route.Detail(it.id)) }, showTitle = true) }
        if (requestable.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Column(Modifier.padding(top = 12.dp)) {
                    Text("Not in your library", style = MaterialTheme.typography.titleMedium)
                    Text("Request it and it will be added", style = MaterialTheme.typography.bodySmall, color = Muted)
                }
            }
            items(requestable, key = { "seerr-${it.mediaType}-${it.tmdbId}" }) { r ->
                Column(Modifier.width(d.posterWidth)) {
                    FocusCard(onClick = {
                        PendingRequest.set(query, r)
                        nav.push(Route.Requests)
                    }) {
                        UrlImage(r.posterUrl, d.posterWidth, Modifier.fillMaxWidth().aspectRatio(2f / 3f))
                        r.availability.label()?.let { Badge(it, Modifier.align(Alignment.TopStart).padding(8.dp)) }
                    }
                    Text(r.title, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(r.year ?: "", style = MaterialTheme.typography.bodySmall, color = Muted)
                }
            }
        }
        if (query.length >= 2 && results.isEmpty() && requestable.isEmpty() && error == null) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("No results for \u201c$query\u201d.", color = Muted) }
        }
    }
}

/** The last search, kept for the process lifetime. */
object SearchMemory {
    var query = ""
}
