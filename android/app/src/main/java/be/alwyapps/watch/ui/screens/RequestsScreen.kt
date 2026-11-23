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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.LibraryTitle
import be.alwyapps.watch.core.seerr.Availability
import be.alwyapps.watch.core.seerr.RequestStatus
import be.alwyapps.watch.core.seerr.SeerrMediaType
import be.alwyapps.watch.core.seerr.SeerrRequest
import be.alwyapps.watch.core.seerr.SeerrResult
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.UrlImage
import be.alwyapps.watch.ui.focusOnAppear
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.rememberSwr
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import kotlinx.coroutines.launch

fun Availability.label(): String? =
    when (this) {
        Availability.AVAILABLE -> tr("Available")
        Availability.PARTIALLY_AVAILABLE -> tr("Partly available")
        // Seerr's "processing" = approved, not on the server yet (not necessarily downloading:
        // see SeerrResult.stateLabel for that).
        Availability.PROCESSING -> tr("Requested")
        Availability.PENDING -> tr("Waiting for approval")
        Availability.BLOCKLISTED -> tr("Blocked")
        Availability.DELETED, Availability.UNKNOWN -> null
    }

/** The precise state of a search result: "Downloading 42%" only when Radarr/Sonarr are on it. */
fun SeerrResult.stateLabel(): String? = downloadProgress?.let { tr("Downloading {percent}%", "percent" to (it * 100).toInt()) } ?: availability.label()

fun RequestStatus.label(): String =
    when (this) {
        RequestStatus.PENDING -> tr("Waiting for approval")
        RequestStatus.APPROVED -> tr("Approved")
        RequestStatus.DECLINED -> tr("Declined")
        RequestStatus.FAILED -> tr("Failed")
        RequestStatus.COMPLETED -> tr("Available")
    }

/**
 * Seerr requests, to follow them: mine, or everyone's (for Seerr users allowed to see them).
 * New requests are made from the global search.
 */
@Composable
fun RequestsScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    var all by rememberSaveable { mutableStateOf(false) }
    val list =
        rememberSwr("seerr-requests", all, cached = { core.cachedSeerrRequests(all) }, refresh = { core.refreshSeerrRequests(it, all) })
    // Where each title is on the servers, looked up ahead (one cached index per server), so a
    // tap opens the playable page at once.
    val account = be.alwyapps.watch.ui.LocalAccount.current
    val places = remember { mutableStateMapOf<String, LibraryTitle?>() }
    LaunchedEffect(list.value) {
        for (r in list.value.orEmpty()) {
            if (r.placeKey() !in places) {
                places[r.placeKey()] = runCatching { core.libraryTitle(r.mediaType, r.tmdbId, account) }.getOrNull()
            }
        }
    }
    LazyVerticalGrid(
        if (d.isTv) GridCells.FixedSize(d.posterWidth) else GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(d.gutter),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                be.alwyapps.watch.ui
                    .PageTitle(tr("Requests"), tr("To ask for something new, search for it."))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ActionButton(tr("Mine"), { all = false }, Modifier.initialFocus(), primary = !all)
                    ActionButton(tr("Everyone"), { all = true }, primary = all)
                }
            }
        }
        list.error?.let { msg ->
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    if (all && list.failure is CoreException.Unauthorized) tr("Only request managers can see everyone's requests.") else msg,
                    color = Muted,
                )
            }
        }
        val items = list.value.orEmpty()
        // Never a blank page: loading, or nothing yet (with how to request).
        if (items.isEmpty() && list.error == null) {
            item(span = { GridItemSpan(maxLineSpan) }) {
                Text(
                    when {
                        list.value == null -> tr("Loading requests…")
                        all -> tr("No requests.")
                        else -> tr("You haven't requested anything yet. Search for a title that isn't on your servers to request it.")
                    },
                    color = Muted,
                )
            }
        }
        items(items, key = { "req-${it.id}" }) { MyRequest(it, showWho = all, place = places[it.placeKey()], known = it.placeKey() in places) }
    }
}

private fun SeerrRequest.placeKey() = "$mediaType/$tmdbId"

@Composable
private fun MyRequest(
    r: SeerrRequest,
    showWho: Boolean = false,
    place: LibraryTitle? = null,
    known: Boolean = false,
) {
    val d = LocalDimens.current
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    val account = be.alwyapps.watch.ui.LocalAccount.current
    val nav = be.alwyapps.watch.ui.LocalNavigator.current
    Column(Modifier.width(d.posterWidth)) {
        // A card (focusable with the remote): the title's page on the server when it is there
        // (to play it), else Seerr's page (its state, seasons, saga).
        be.alwyapps.watch.ui.FocusCard(onClick = {
            scope.launch {
                val at = if (known) place else runCatching { core.libraryTitle(r.mediaType, r.tmdbId, account) }.getOrNull()
                nav.push(
                    if (at != null) {
                        be.alwyapps.watch.ui.Route
                            .Detail(at.itemId, account = at.accountId)
                    } else {
                        be.alwyapps.watch.ui.Route
                            .SeerrTitle(if (r.mediaType == SeerrMediaType.TV) "tv" else "movie", r.tmdbId)
                    },
                )
            }
        }) {
            UrlImage(r.posterUrl, d.posterWidth, Modifier.fillMaxWidth().aspectRatio(2f / 3f))
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
        Text(
            listOfNotNull(
                r.requestedBy.takeIf { showWho && it.isNotEmpty() },
                r.createdAt.take(10),
            ).joinToString(" · "),
            color = Muted,
            style = MaterialTheme.typography.bodySmall,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

/** Request a movie, or pick seasons of a show (shown by the global search). */
@Composable
fun RequestPanel(
    r: SeerrResult,
    onDone: (String) -> Unit,
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    // Shows: the seasons with their state (on the server, requested…), to pick the missing ones.
    var seasons by remember(r.tmdbId) { mutableStateOf<List<be.alwyapps.watch.core.seerr.SeerrSeasonInfo>?>(null) }
    var chosen by remember(r.tmdbId) { mutableStateOf(setOf<UInt>()) }
    var busy by remember { mutableStateOf(false) }
    if (r.mediaType == SeerrMediaType.TV) {
        LaunchedEffect(r.tmdbId) { seasons = runCatching { core.seerrSeasonInfo(r.tmdbId) }.getOrNull() }
    }

    fun send(numbers: List<UInt>) {
        if (busy) return
        busy = true
        scope.launch {
            val msg =
                try {
                    core.seerrRequest(r.mediaType, r.tmdbId, numbers)
                    if (numbers.isEmpty()) {
                        tr("Requested: {title}", "title" to r.title)
                    } else {
                        be.alwyapps.watch.core
                            .seasonsRequestedMessage(numbers)
                    }
                } catch (e: CoreException) {
                    tr("Could not request {title}: {reason}", "title" to r.title, "reason" to requestErrorText(e))
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
                        listOfNotNull(r.year, if (r.mediaType == SeerrMediaType.TV) tr("Show") else tr("Movie"), r.stateLabel()).joinToString(" · "),
                        color = Muted,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                be.alwyapps.watch.ui
                    .IconAction(be.alwyapps.watch.ui.Icons.Close, tr("Close"), { onDone("") })
            }
            r.overview?.let { Text(it, color = Muted, maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium) }
            if (r.mediaType == SeerrMediaType.MOVIE) {
                if (r.availability != Availability.AVAILABLE) {
                    ActionButton(
                        if (busy) tr("Requesting…") else tr("Request"),
                        { send(emptyList()) },
                        Modifier.focusOnAppear(),
                        primary = true,
                        icon = be.alwyapps.watch.ui.Icons.Add,
                    )
                }
            } else {
                val info = seasons
                if (info == null) {
                    Text(tr("Loading seasons…"), color = Muted)
                } else {
                    val plan =
                        remember(info) {
                            be.alwyapps.watch.core
                                .seasonPlan(info, emptyList())
                        }
                    if (plan.missing.isEmpty()) {
                        Text(tr("Every season is on your servers or already requested."), color = Muted)
                    } else {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            ActionButton(
                                when {
                                    busy -> tr("Requesting…")
                                    chosen.isNotEmpty() -> trn("Request {n} season", "Request {n} seasons", chosen.size)
                                    plan.hasSome -> tr("Request the missing seasons")
                                    else -> tr("Request all seasons")
                                },
                                // Nothing picked: the missing ones (every season of a new show).
                                {
                                    send(
                                        if (chosen.isNotEmpty()) {
                                            chosen.sorted()
                                        } else if (plan.hasSome) {
                                            plan.missing
                                        } else {
                                            emptyList()
                                        },
                                    )
                                },
                                Modifier.focusOnAppear(),
                                primary = true,
                                icon = be.alwyapps.watch.ui.Icons.Add,
                            )
                        }
                    }
                    androidx.compose.foundation.lazy.LazyRow(
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        items(plan.seasons.size) { i ->
                            val s = plan.seasons[i]
                            if (s.state == be.alwyapps.watch.core.seerr.SeasonState.REQUESTABLE) {
                                ActionButton(
                                    s.name,
                                    { chosen = if (s.number in chosen) chosen - s.number else chosen + s.number },
                                    primary = s.number in chosen,
                                    icon = if (s.number in chosen) be.alwyapps.watch.ui.Icons.Check else null,
                                )
                            } else {
                                // On the server or asked for already: said, not selectable.
                                be.alwyapps.watch.ui
                                    .Badge("${s.name} · ${s.label}", color = be.alwyapps.watch.ui.SurfaceHigh)
                            }
                        }
                    }
                }
            }
        }
    }
}
