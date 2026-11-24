// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.GridItemSpan
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.seerr.Availability
import be.alwyapps.watch.core.seerr.SeerrCollection
import be.alwyapps.watch.core.seerr.SeerrDetails
import be.alwyapps.watch.core.seerr.SeerrMediaType
import be.alwyapps.watch.core.seerr.SeerrResult
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Badge
import be.alwyapps.watch.ui.ErrorBanner
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.Success
import be.alwyapps.watch.ui.UrlImage
import be.alwyapps.watch.ui.dpadScroll
import be.alwyapps.watch.ui.fadeEdges
import be.alwyapps.watch.ui.focusOnAppear
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn

/** The state of a title as one label ("Available", "Requested", "Downloading 42%"…). */
private fun SeerrDetails.stateLabel(): String? = downloadProgress?.let { tr("Downloading {percent}%", "percent" to (it * 100).toInt()) } ?: availability.label()

/** The search-result form of a title, for the request panel. */
private fun SeerrDetails.asResult() = SeerrResult(tmdbId, mediaType, title, year, overview, posterUrl, availability, downloadProgress)

/**
 * A title that is not (or not fully) on the servers, like Seerr's own page: artwork, details,
 * state, request, cast, and the saga it belongs to.
 */
@Composable
fun SeerrTitleScreen(
    mediaType: String,
    tmdbId: Long,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val type = if (mediaType == "tv") SeerrMediaType.TV else SeerrMediaType.MOVIE
    var reload by remember { mutableIntStateOf(0) }
    var title by remember { mutableStateOf<SeerrDetails?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var requesting by remember { mutableStateOf(false) }
    var message by remember { mutableStateOf<String?>(null) }
    // Shows: season by season (some may be on the server or requested while others are not).
    var seasonPlan by remember { mutableStateOf<be.alwyapps.watch.core.seerr.SeasonPlan?>(null) }
    LaunchedEffect(reload) {
        try {
            title = core.seerrTitle(type, tmdbId)
            error = null
        } catch (e: CoreException) {
            error = e.message
        }
        if (type == SeerrMediaType.TV) {
            runCatching { core.seerrSeasonInfo(tmdbId) }.onSuccess {
                seasonPlan =
                    be.alwyapps.watch.core
                        .seasonPlan(it, emptyList())
            }
        }
    }
    // TV: the overview, cast… have nothing to focus: ↓ past the last button scrolls the page.
    val list =
        androidx.compose.foundation.lazy
            .rememberLazyListState()
    LazyColumn(
        Modifier.fillMaxSize().dpadScroll(list),
        state = list,
        contentPadding = PaddingValues(bottom = 40.dp),
    ) {
        item(key = "hero") {
            // TV: short enough that the title, its state and the Request button (or why not) are
            // on the first screen (420 dp left them below the fold of a 540 dp-high TV).
            Box(Modifier.fillMaxWidth().height(if (d.isTv) 200.dp else 300.dp)) {
                UrlImage(title?.backdropUrl, 1280.dp, Modifier.fillMaxSize().fadeEdges(bottomFrom = 0.45f))
                Box(Modifier.fillMaxSize().background(Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.5f), 0.25f to Color.Transparent)))
                if (!d.isTv) {
                    IconAction(Icons.Back, tr("Back"), { nav.pop() }, Modifier.align(Alignment.TopStart).statusBarsPadding().padding(12.dp))
                }
            }
        }
        error?.let { msg -> item { ErrorBanner(msg) { reload++ } } }
        val t = title ?: return@LazyColumn
        item(key = "head") {
            Row(Modifier.padding(horizontal = d.gutter), horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                UrlImage(t.posterUrl, 160.dp, Modifier.width(if (d.isTv) 150.dp else 110.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(d.radius)))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(t.title, style = MaterialTheme.typography.headlineMedium, maxLines = 3, overflow = TextOverflow.Ellipsis)
                    Text(
                        listOfNotNull(
                            t.year,
                            if (t.mediaType == SeerrMediaType.TV) t.seasons?.let { trn("{n} season", "{n} seasons", it.toLong()) } ?: tr("Show") else null,
                            t.runtimeMin?.let { formatRuntimeMin(it.toInt()) },
                            t.rating?.let { "★ %.1f".format(it) },
                        ).joinToString(" · "),
                        color = Muted,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    t.stateLabel()?.let {
                        Badge(
                            it,
                            color = if (t.availability == Availability.AVAILABLE) Success else be.alwyapps.watch.ui.SurfaceHigh,
                            textColor = if (t.availability == Availability.AVAILABLE) Color.Black else Color.White,
                        )
                    }
                    if (d.isTv && !requesting) PrimaryAction(t, seasonPlan, onRequest = { requesting = true }, onBack = { nav.pop() })
                }
            }
        }
        item(key = "actions") {
            Column(Modifier.padding(horizontal = d.gutter, vertical = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (!d.isTv && !requesting) PrimaryAction(t, seasonPlan, onRequest = { requesting = true }, onBack = { nav.pop() })
                if (requesting) {
                    RequestPanel(t.asResult(), onDone = { msg ->
                        requesting = false
                        message = msg.ifEmpty { null }
                        reload++
                    })
                }
                message?.let { Text(it, color = Accent) }
                t.tagline?.let { Text(it, style = MaterialTheme.typography.titleMedium) }
                t.overview?.let {
                    Text(
                        it,
                        style = MaterialTheme.typography.bodyLarge,
                        color =
                            be.alwyapps.watch.ui.OnSurface
                                .copy(alpha = 0.85f),
                    )
                }
                if (t.genres.isNotEmpty()) {
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        t.genres.take(5).forEach { Badge(it, color = be.alwyapps.watch.ui.SurfaceHigh) }
                    }
                }
            }
        }
        t.collection?.let { c ->
            item(key = "saga") {
                Column(Modifier.padding(horizontal = d.gutter, vertical = 8.dp)) {
                    FocusCard(onClick = { nav.push(Route.SeerrSaga(c.id, c.name)) }, glow = false) {
                        Row(
                            Modifier.fillMaxWidth().background(be.alwyapps.watch.ui.SurfaceColor).padding(16.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Column(Modifier.weight(1f)) {
                                Text(tr("PART OF THE SAGA"), style = MaterialTheme.typography.labelSmall, color = Accent)
                                Text(c.name, style = MaterialTheme.typography.titleMedium)
                            }
                            be.alwyapps.watch.ui
                                .Chevron()
                        }
                    }
                }
            }
        }
        if (t.cast.isNotEmpty()) {
            item(key = "cast") {
                Column(Modifier.padding(top = 16.dp)) {
                    Text(tr("Cast"), Modifier.padding(horizontal = d.gutter), style = MaterialTheme.typography.titleMedium)
                    LazyRow(contentPadding = PaddingValues(horizontal = d.gutter, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        items(t.cast) { p ->
                            Column(Modifier.width(88.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                                UrlImage(p.photoUrl, 120.dp, Modifier.size(76.dp).clip(CircleShape))
                                Text(
                                    p.name,
                                    Modifier.padding(top = 6.dp),
                                    style = MaterialTheme.typography.bodySmall,
                                    maxLines = 2,
                                    overflow = TextOverflow.Ellipsis,
                                )
                                p.character?.let {
                                    Text(
                                        it,
                                        style = MaterialTheme.typography.bodySmall,
                                        color = Subtle,
                                        maxLines = 1,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                }
                            }
                        }
                    }
                }
            }
        }
        item { Spacer(Modifier.height(24.dp)) }
    }
}

/**
 * Whether the title can be requested now (not on the servers, or a show with seasons missing).
 * Shows: their seasons decide once known (a show partly requested still has others to ask for).
 */
private fun SeerrDetails.canRequest(plan: be.alwyapps.watch.core.seerr.SeasonPlan?) =
    if (mediaType == SeerrMediaType.TV && plan != null) plan.missing.isNotEmpty() else canRequestTitle()

private fun SeerrDetails.canRequestTitle() =
    availability == Availability.UNKNOWN ||
        availability == Availability.DELETED ||
        (mediaType == SeerrMediaType.TV && availability == Availability.PARTIALLY_AVAILABLE)

/** Why there is no Request button, in one line. */
private fun SeerrDetails.whyNoRequest(plan: be.alwyapps.watch.core.seerr.SeasonPlan?): String =
    when {
        plan != null && mediaType == SeerrMediaType.TV && availability != Availability.AVAILABLE ->
            tr("Every season is on your servers or already requested.")
        downloadProgress != null -> tr("Already requested: it is downloading.")
        availability == Availability.AVAILABLE -> tr("Already on your servers: search for it to watch it.")
        availability == Availability.PARTIALLY_AVAILABLE -> tr("Already partly on your servers.")
        availability == Availability.PROCESSING -> tr("Already requested and approved: it will be added soon.")
        availability == Availability.PENDING -> tr("Already requested: waiting for approval.")
        availability == Availability.BLOCKLISTED -> tr("Blocked on the request server: it can't be requested.")
        else -> tr("It can't be requested.")
    }

/**
 * The page's main element, always there and focused (TV): Request, or why not with a way back
 * (a page with nothing to focus left the D-pad dead).
 */
@Composable
private fun PrimaryAction(
    t: SeerrDetails,
    plan: be.alwyapps.watch.core.seerr.SeasonPlan?,
    onRequest: () -> Unit,
    onBack: () -> Unit,
) {
    val d = LocalDimens.current
    // focusOnAppear: it also comes back in place of the request panel (closed, or request sent).
    if (t.canRequest(plan)) {
        val label = if (t.mediaType == SeerrMediaType.TV && plan?.hasSome == true) tr("Request the missing seasons") else tr("Request")
        ActionButton(label, onRequest, Modifier.then(if (d.isTv) Modifier else Modifier.fillMaxWidth()).focusOnAppear(), primary = true, icon = Icons.Add)
    } else {
        Text(t.whyNoRequest(plan), color = Muted, style = MaterialTheme.typography.bodyMedium)
        if (d.isTv) ActionButton(tr("Back"), onBack, Modifier.focusOnAppear(), icon = Icons.Back)
    }
}

private fun formatRuntimeMin(min: Int) =
    if (min >= 60) tr("{hours} h {minutes} min", "hours" to min / 60, "minutes" to min % 60) else tr("{minutes} min", "minutes" to min)

/** A saga: its films with their state; each opens its page. */
@Composable
fun SeerrSagaScreen(
    id: Long,
    name: String,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    var saga by remember { mutableStateOf<SeerrCollection?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var reload by remember { mutableIntStateOf(0) }
    LaunchedEffect(reload) {
        try {
            saga = core.seerrCollection(id)
            error = null
        } catch (e: CoreException) {
            error = e.message
        }
    }
    LazyVerticalGrid(
        if (d.isTv) GridCells.FixedSize(d.posterWidth) else GridCells.Adaptive(d.posterWidth + 8.dp),
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(d.gutter),
        horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        item(span = { GridItemSpan(maxLineSpan) }) {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                be.alwyapps.watch.ui
                    .PageTitle(saga?.name ?: name, saga?.parts?.let { trn("{n} film", "{n} films", it.size) })
                saga?.overview?.let { Text(it, color = Muted, style = MaterialTheme.typography.bodyMedium) }
            }
        }
        error?.let { msg -> item(span = { GridItemSpan(maxLineSpan) }) { ErrorBanner(msg) { reload++ } } }
        items(saga?.parts.orEmpty(), key = { it.tmdbId }) { r -> SeerrPoster(r) { nav.push(Route.SeerrTitle("movie", r.tmdbId)) } }
    }
}

/** A Seerr title as a poster with its state (search, sagas). */
@Composable
fun SeerrPoster(
    r: SeerrResult,
    cardModifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val d = LocalDimens.current
    Column(Modifier.width(d.posterWidth)) {
        FocusCard(onClick = onClick, modifier = cardModifier) {
            UrlImage(r.posterUrl, d.posterWidth, Modifier.fillMaxWidth().aspectRatio(2f / 3f))
            // No poster on TMDB: the title in its place (the one below scrolls off on TV rows).
            if (r.posterUrl == null) {
                Text(
                    r.title,
                    Modifier.align(Alignment.Center).padding(12.dp),
                    style = MaterialTheme.typography.titleSmall,
                    color = Muted,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                    maxLines = 4,
                    overflow = TextOverflow.Ellipsis,
                )
            }
            r.stateLabel()?.let {
                Badge(
                    it,
                    Modifier.align(Alignment.TopStart).padding(8.dp),
                    color = if (r.availability == Availability.AVAILABLE) Success else Color.Black.copy(alpha = 0.6f),
                    textColor = if (r.availability == Availability.AVAILABLE) Color.Black else Color.White,
                )
            }
        }
        Text(r.title, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(r.year ?: "", style = MaterialTheme.typography.bodySmall, color = Muted)
    }
}
