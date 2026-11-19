// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.seerr.Availability
import be.alwyapps.watch.core.seerr.RequestStatus
import be.alwyapps.watch.core.seerr.SeerrMediaType
import be.alwyapps.watch.core.seerr.SeerrRequest
import be.alwyapps.watch.core.seerr.SeerrResult
import be.alwyapps.watch.core.seerr.SeerrSeason
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.UrlImage
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.rememberSwr
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

fun Availability.label(): String? =
    when (this) {
        Availability.AVAILABLE -> "Available"
        Availability.PARTIALLY_AVAILABLE -> "Partly available"
        Availability.PROCESSING -> "Downloading"
        Availability.PENDING -> "Requested"
        Availability.BLOCKLISTED -> "Blocked"
        Availability.DELETED, Availability.UNKNOWN -> null
    }

fun RequestStatus.label(): String =
    when (this) {
        RequestStatus.PENDING -> "Waiting for approval"
        RequestStatus.APPROVED -> "Approved"
        RequestStatus.DECLINED -> "Declined"
        RequestStatus.FAILED -> "Failed"
        RequestStatus.COMPLETED -> "Available"
    }

/** Seerr: search TMDB, request movies or seasons, follow my requests. */
@Composable
fun RequestsScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    var query by rememberSaveable { mutableStateOf("") }
    var results by remember { mutableStateOf<List<SeerrResult>>(emptyList()) }
    var selected by remember { mutableStateOf<SeerrResult?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    val mine = rememberSwr("seerr-requests", cached = { core.cachedSeerrRequests() }, refresh = { core.refreshSeerrRequests(it) })
    LaunchedEffect(query) {
        if (query.length < 2) {
            results = emptyList()
            return@LaunchedEffect
        }
        delay(350)
        try {
            results = core.seerrSearch(query)
            message = null
        } catch (e: CoreException) {
            message = e.message
        }
    }
    LazyVerticalGrid(
        GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(d.gutter),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            OutlinedTextField(
                query,
                { query = it },
                Modifier.fillMaxWidth().dpadEscape(),
                label = { Text("Find a movie or a show to request") },
                singleLine = true,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            )
        }
        message?.let { item(span = { GridItemSpan(maxLineSpan) }) { Text(it, color = Accent) } }
        selected?.let { r ->
            item(span = { GridItemSpan(maxLineSpan) }, key = "sel-${r.tmdbId}") {
                RequestPanel(r, onDone = { msg ->
                    message = msg
                    selected = null
                    mine.reload()
                })
            }
        }
        items(results, key = { "${it.mediaType}-${it.tmdbId}" }) { r ->
            Column(Modifier.width(d.posterWidth)) {
                FocusCard(onClick = { selected = r }) {
                    UrlImage(r.posterUrl, d.posterWidth, Modifier.fillMaxWidth().aspectRatio(2f / 3f))
                }
                Text(r.title, Modifier.padding(top = 6.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(listOfNotNull(r.year, r.availability.label()).joinToString(" · "), color = Muted, style = MaterialTheme.typography.bodySmall)
            }
        }
        if (query.length < 2) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text("My requests", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
            }
            mine.error?.let { item(span = { GridItemSpan(maxLineSpan) }) { Text(it, color = Muted) } }
            val list = mine.value.orEmpty()
            if (list.isEmpty() && mine.value != null) {
                item(span = { GridItemSpan(maxLineSpan) }) { Text("No requests yet.", color = Muted) }
            }
            items(list, key = { "req-${it.id}" }) { MyRequest(it) }
        }
    }
}

@Composable
private fun MyRequest(r: SeerrRequest) {
    val d = LocalDimens.current
    Column(Modifier.width(d.posterWidth)) {
        UrlImage(r.posterUrl, d.posterWidth, Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(8.dp)))
        Text(r.title ?: "TMDB ${r.tmdbId}", Modifier.padding(top = 6.dp), maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(r.status.label(), color = if (r.status == RequestStatus.COMPLETED) Accent else Muted, style = MaterialTheme.typography.bodySmall)
    }
}

/** Request a movie, or pick seasons of a show. */
@Composable
private fun RequestPanel(
    r: SeerrResult,
    onDone: (String) -> Unit,
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var seasons by remember(r.tmdbId) { mutableStateOf<List<SeerrSeason>?>(null) }
    var chosen by remember(r.tmdbId) { mutableStateOf(setOf<UInt>()) }
    var busy by remember { mutableStateOf(false) }
    if (r.mediaType == SeerrMediaType.TV) {
        LaunchedEffect(r.tmdbId) { seasons = runCatching { core.seerrSeasons(r.tmdbId) }.getOrNull() }
    }

    fun send(numbers: List<UInt>) {
        if (busy) return
        busy = true
        scope.launch {
            val msg =
                try {
                    core.seerrRequest(r.mediaType, r.tmdbId, numbers)
                    "Requested: ${r.title}"
                } catch (e: CoreException) {
                    "Could not request ${r.title}: ${e.message}"
                }
            busy = false
            onDone(msg)
        }
    }
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(8.dp))
            .background(SurfaceColor)
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(listOfNotNull(r.title, r.year).joinToString(" · "), style = MaterialTheme.typography.titleMedium)
        r.overview?.let { Text(it, color = Muted, maxLines = 4, overflow = TextOverflow.Ellipsis) }
        r.availability.label()?.let { Text(it, color = Accent) }
        if (r.mediaType == SeerrMediaType.MOVIE) {
            if (r.availability != Availability.AVAILABLE) ActionButton(if (busy) "Requesting…" else "Request", { send(emptyList()) }, primary = true)
        } else {
            val list = seasons
            if (list == null) {
                Text("Loading seasons…", color = Muted)
            } else {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    ActionButton(if (busy) "Requesting…" else "Request all seasons", { send(emptyList()) }, primary = chosen.isEmpty())
                    if (chosen.isNotEmpty()) ActionButton("Request ${chosen.size} selected", { send(chosen.sorted()) }, primary = true)
                }
                for (s in list) {
                    val status = s.availability.label()
                    val requestable = s.availability == Availability.UNKNOWN || s.availability == Availability.DELETED
                    ActionButton(
                        (if (s.number in chosen) "✓ " else "") + s.name + (status?.let { " · $it" } ?: ""),
                        { if (requestable) chosen = if (s.number in chosen) chosen - s.number else chosen + s.number },
                        primary = s.number in chosen,
                    )
                }
            }
        }
        ActionButton("Close", { onDone("") })
    }
}
