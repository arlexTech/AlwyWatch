// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.seerr.SeerrResult
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.PosterCard
import be.alwyapps.watch.ui.Route
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
    // Library results with the account they come from (all servers when shown together).
    var results by remember { mutableStateOf<List<Pair<String, Item>>>(emptyList()) }
    var requestable by remember { mutableStateOf<List<SeerrResult>>(emptyList()) }
    var error by remember { mutableStateOf<String?>(null) }
    var seerrError by remember { mutableStateOf<String?>(null) }
    // All (default), on my servers only, or only what is not on them.
    var filter by rememberSaveable { mutableStateOf("all") }
    val seerrOn = remember { core.seerrActive() }
    val focus = remember { FocusRequester() }
    // Keyboard straight away only for a new search, not when coming back to results.
    LaunchedEffect(Unit) { if (query.isEmpty()) runCatching { focus.requestFocus() } }
    // Watched state changed from a result's menu: search again so the marks follow.
    val changed = be.alwyapps.watch.ui.ItemsChanged.version
    LaunchedEffect(query, changed) {
        if (query.length < 2) {
            results = emptyList()
            requestable = emptyList()
            return@LaunchedEffect
        }
        delay(300)
        val accounts = runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id }
        val together = accounts.size > 1 && runCatching { core.getSetting(COMBINED_KEY) }.getOrNull() != "0"
        val found = mutableListOf<Pair<String, Item>>()
        var failed: String? = null
        // Preferred server first: its copy wins when a title is on several.
        for (a in if (together) preferredFirst(core, accounts) else listOf(account)) {
            try {
                core.search(a, query, 60u).forEach { found += a to it }
            } catch (e: CoreException) {
                failed = e.message
            }
        }
        // One per title across servers.
        results = found.distinctBy { (_, i) -> i.sameAs() }
        error = failed.takeIf { found.isEmpty() }
        if (seerrOn) {
            try {
                // Everything that exists, minus what the library results already show.
                val have = results.mapNotNull { (_, i) -> i.providerIds.tmdb }.toSet()
                val norm = { t: String -> t.lowercase().filter { it.isLetterOrDigit() } }
                val names = results.map { (_, i) -> norm(i.title) }.toSet()
                requestable = core.seerrSearch(query).filter { it.tmdbId.toString() !in have && norm(it.title) !in names }.take(30)
                seerrError = null
            } catch (e: CoreException) {
                requestable = emptyList()
                seerrError = e.message
            }
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
        if (query.length >= 2) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Row(
                    Modifier.horizontalScroll(androidx.compose.foundation.rememberScrollState()),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    for ((key, label) in listOf("all" to "All", "mine" to "On my servers", "other" to "Not on my servers")) {
                        be.alwyapps.watch.ui
                            .ActionButton(label, { filter = key }, primary = filter == key)
                    }
                }
            }
        }
        error?.let { item(span = { GridItemSpan(maxLineSpan) }) { Text(it, color = Muted) } }
        val showMine = filter != "other"
        val showOther = filter != "mine"
        if (showMine && results.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("On your servers", style = MaterialTheme.typography.titleMedium) }
        }
        if (showMine) {
            items(results, key = { (_, i) -> i.id }) { (a, i) ->
                androidx.compose.runtime.CompositionLocalProvider(
                    be.alwyapps.watch.ui.LocalItemOwners provides mapOf(i.id to a),
                    be.alwyapps.watch.ui.LocalServerBadges provides (results.map { it.first }.distinct().size > 1),
                ) {
                    PosterCard(i, d.posterWidth, onClick = { nav.push(Route.Detail(i.id, account = a)) }, showTitle = true)
                }
            }
        }
        if (showOther && query.length >= 2 && !seerrOn) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    "To also find what is not on your servers (and request it), connect requests in Settings.",
                    Modifier.padding(top = 8.dp),
                    color = Muted,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }
        seerrError?.takeIf { showOther }?.let { msg ->
            item(span = { GridItemSpan(maxLineSpan) }) { Text("Requests unavailable: $msg", color = Muted, style = MaterialTheme.typography.bodySmall) }
        }
        if (showOther && requestable.isNotEmpty()) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Column(Modifier.padding(top = 12.dp)) {
                    Text("Not on your servers", style = MaterialTheme.typography.titleMedium)
                    Text("Open one to see it and request it", style = MaterialTheme.typography.bodySmall, color = Muted)
                }
            }
            items(requestable, key = { "seerr-${it.mediaType}-${it.tmdbId}" }) { r ->
                SeerrPoster(r) { nav.push(Route.SeerrTitle(if (r.mediaType == be.alwyapps.watch.core.seerr.SeerrMediaType.TV) "tv" else "movie", r.tmdbId)) }
            }
        }
        if (query.length >= 2 && (!showMine || results.isEmpty()) && (!showOther || requestable.isEmpty()) && error == null) {
            item(span = { GridItemSpan(maxLineSpan) }) { Text("No results for \u201c$query\u201d.", color = Muted) }
        }
    }
}

/** The last search, kept for the process lifetime. */
object SearchMemory {
    var query = ""
}
