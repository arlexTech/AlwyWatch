// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
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
import androidx.compose.ui.graphics.Color
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
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.UrlImage
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.initialFocus
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

/** Search hands a Seerr result over to the requests screen (query + chosen title). */
object PendingRequest {
    private var value: Pair<String, SeerrResult>? = null

    fun set(
        query: String,
        result: SeerrResult,
    ) {
        value = query to result
    }

    fun take(): Pair<String, SeerrResult>? = value.also { value = null }
}

/** Seerr: search TMDB, request movies or seasons, follow my requests. */
@Composable
fun RequestsScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val handoff = remember { PendingRequest.take() }
    var query by rememberSaveable { mutableStateOf(handoff?.first ?: "") }
    var results by remember { mutableStateOf<List<SeerrResult>>(emptyList()) }
    var selected by remember { mutableStateOf<SeerrResult?>(handoff?.second) }
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
            Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                be.alwyapps.watch.ui
                    .PageTitle("Requests", "Ask for a movie or a show; it is added when ready.")
                Field(
                    query,
                    { query = it },
                    Modifier.fillMaxWidth().dpadEscape(),
                    placeholder = { Text("Find a movie or a show…") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                    leadingIcon = be.alwyapps.watch.ui.Icons.Search,
                    pill = true,
                )
            }
        }
        message?.takeIf { it.isNotEmpty() }?.let {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    it,
                    Modifier
                        .clip(RoundedCornerShape(12.dp))
                        .background(be.alwyapps.watch.ui.SurfaceHigh)
                        .padding(horizontal = 14.dp, vertical = 10.dp),
                    color = Accent,
                )
            }
        }
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
                    r.availability.label()?.let {
                        be.alwyapps.watch.ui.Badge(
                            it,
                            Modifier.align(Alignment.TopStart).padding(8.dp),
                            color = if (r.availability == Availability.AVAILABLE) be.alwyapps.watch.ui.Success else Color.Black.copy(alpha = 0.6f),
                            textColor = if (r.availability == Availability.AVAILABLE) Color.Black else Color.White,
                        )
                    }
                }
                Text(r.title, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(r.year ?: "", color = Muted, style = MaterialTheme.typography.bodySmall)
            }
        }
        if (query.length < 2) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text("My requests", Modifier.padding(top = 4.dp), style = MaterialTheme.typography.titleMedium)
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
        Box(Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(d.radius))) {
            UrlImage(r.posterUrl, d.posterWidth, Modifier.fillMaxSize())
            val (bg, fg) =
                when (r.status) {
                    RequestStatus.COMPLETED -> be.alwyapps.watch.ui.Success to Color.Black
                    RequestStatus.APPROVED -> Color(0xFF5B8CFF) to Color.White
                    RequestStatus.DECLINED, RequestStatus.FAILED -> be.alwyapps.watch.ui.Danger to Color.White
                    RequestStatus.PENDING -> Accent to Color.Black
                }
            be.alwyapps.watch.ui
                .Badge(r.status.label(), Modifier.align(Alignment.TopStart).padding(8.dp), color = bg, textColor = fg)
        }
        Text(
            r.title ?: "TMDB ${r.tmdbId}",
            Modifier.padding(top = 8.dp),
            style = MaterialTheme.typography.titleSmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(r.createdAt.take(10), color = Muted, style = MaterialTheme.typography.bodySmall)
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
    val d = LocalDimens.current
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(d.radius + 6.dp))
            .background(be.alwyapps.watch.ui.SurfaceHigh)
            .padding(16.dp),
        horizontalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        UrlImage(r.posterUrl, 160.dp, Modifier.width(if (d.isTv) 160.dp else 110.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(d.radius)))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.Top) {
                Column(Modifier.weight(1f)) {
                    Text(r.title, style = MaterialTheme.typography.headlineMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOfNotNull(r.year, if (r.mediaType == SeerrMediaType.TV) "Show" else "Movie", r.availability.label()).joinToString(" · "),
                        color = Muted,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                be.alwyapps.watch.ui
                    .IconAction(be.alwyapps.watch.ui.Icons.Close, "Close", { onDone("") })
            }
            r.overview?.let { Text(it, color = Muted, maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium) }
            if (r.mediaType == SeerrMediaType.MOVIE) {
                if (r.availability != Availability.AVAILABLE) {
                    ActionButton(
                        if (busy) "Requesting…" else "Request",
                        { send(emptyList()) },
                        Modifier.initialFocus(),
                        primary = true,
                        icon = be.alwyapps.watch.ui.Icons.Add,
                    )
                }
            } else {
                val list = seasons
                if (list == null) {
                    Text("Loading seasons…", color = Muted)
                } else {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        ActionButton(
                            if (busy) {
                                "Requesting…"
                            } else if (chosen.isEmpty()) {
                                "Request all seasons"
                            } else {
                                "Request ${chosen.size} season${if (chosen.size > 1) "s" else ""}"
                            },
                            { send(chosen.sorted()) },
                            Modifier.initialFocus(),
                            primary = true,
                            icon = be.alwyapps.watch.ui.Icons.Add,
                        )
                    }
                    androidx.compose.foundation.lazy.LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        items(list.size) { i ->
                            val s = list[i]
                            val requestable = s.availability == Availability.UNKNOWN || s.availability == Availability.DELETED
                            ActionButton(
                                s.name + (s.availability.label()?.let { " · $it" } ?: ""),
                                { if (requestable) chosen = if (s.number in chosen) chosen - s.number else chosen + s.number },
                                primary = s.number in chosen,
                                icon = if (s.number in chosen) be.alwyapps.watch.ui.Icons.Check else null,
                            )
                        }
                    }
                }
            }
        }
    }
}
