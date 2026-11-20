// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.downloads.DeleteRule
import be.alwyapps.watch.core.downloads.Download
import be.alwyapps.watch.core.downloads.DownloadState
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind
import be.alwyapps.watch.downloads.DownloadWorker
import be.alwyapps.watch.player.PlayerActivity
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Badge
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.RemoteImage
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.SectionHeader
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.SurfaceHigh
import be.alwyapps.watch.ui.poster
import be.alwyapps.watch.ui.thumb
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

// ----- Rules ----------------------------------------------------------------------------------

/** The deletion rules offered, in order. */
val RulePresets: List<DeleteRule> =
    listOf(
        DeleteRule.Never,
        DeleteRule.WhenWatched,
        DeleteRule.DaysAfterWatched(1u),
        DeleteRule.DaysAfterWatched(7u),
        DeleteRule.AfterDays(7u),
        DeleteRule.AfterDays(30u),
    )

fun DeleteRule.label(): String =
    when (this) {
        is DeleteRule.Never -> "Keep"
        is DeleteRule.WhenWatched -> "Once watched"
        is DeleteRule.DaysAfterWatched -> if (days == 1u) "1 day after watching" else "$days days after watching"
        is DeleteRule.AfterDays -> "After $days days"
    }

@Composable
fun RuleChips(
    selected: DeleteRule,
    onSelect: (DeleteRule) -> Unit,
) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (r in RulePresets) ActionButton(r.label(), { onSelect(r) }, primary = r == selected)
    }
}

// ----- Download data helpers ------------------------------------------------------------------

private val Download.done get() = state == DownloadState.COMPLETED

/** Show of a download: from its metadata, or parsed from older titles ("Show · S1 · E2 · …"). */
private fun Download.showKey(): Pair<String, String>? {
    episode?.let { return it.seriesId to it.seriesTitle }
    val parts = title.split(" · ")
    return if (parts.size >= 3 && parts[1].startsWith("S")) ("title:" + parts[0]) to parts[0] else null
}

private fun Download.seasonEpisode(): Pair<Int, Int> {
    episode?.let { return (it.seasonNumber?.toInt() ?: 0) to (it.episodeNumber?.toInt() ?: 0) }
    val parts = title.split(" · ")
    return (parts.getOrNull(1)?.drop(1)?.toIntOrNull() ?: 0) to (parts.getOrNull(2)?.drop(1)?.toIntOrNull() ?: 0)
}

private fun Download.episodeTitle(): String = title.split(" · ").last()

private fun gb(b: ULong) = "%.1f GB".format(b.toDouble() / 1e9)

fun Download.status(): String {
    val size = total?.let { "${gb(downloaded)} / ${gb(it)}" } ?: gb(downloaded)
    return when (state) {
        DownloadState.QUEUED -> "Queued · $size"
        DownloadState.RUNNING -> "Downloading · $size"
        DownloadState.PAUSED -> "Paused · $size"
        DownloadState.PAUSED_NO_SPACE -> "Paused: storage full. Free some space, then resume."
        DownloadState.WAITING -> "Waiting for the network · $size"
        DownloadState.FAILED -> "Failed: ${error ?: "unknown error"}"
        DownloadState.COMPLETED ->
            listOfNotNull(
                total?.let(::gb),
                if (watchedAt != null) "watched" else null,
                deleteRule.takeIf { it !is DeleteRule.Never }?.let { "deletes ${it.label().lowercase()}" },
            ).joinToString(" · ")
    }
}

/** Live list of downloads (polled once a second while shown). */
@Composable
fun rememberDownloads(): Pair<List<Download>, () -> Unit> {
    val core = LocalCore.current
    var list by remember { mutableStateOf(runCatching { core.downloads() }.getOrDefault(emptyList())) }
    var tick by remember { mutableIntStateOf(0) }
    LaunchedEffect(tick) {
        while (true) {
            list = runCatching { core.downloads() }.getOrDefault(list)
            delay(1000)
        }
    }
    return list to { tick++ }
}

/**
 * Items (or shows) whose download was just asked for: buttons show a spinner at once, before
 * the server answers with the file to fetch. [changed] makes download lists re-read now.
 */
object DownloadPending {
    private val ids = androidx.compose.runtime.mutableStateListOf<String>()
    var version by mutableIntStateOf(0)
        private set

    fun contains(id: String) = id in ids

    fun add(id: String) {
        ids.add(id)
        version++
    }

    fun remove(id: String) {
        ids.remove(id)
        version++
    }

    fun changed() {
        version++
    }
}

/**
 * This account's downloads by item id, kept current: every second while something downloads,
 * every few seconds otherwise (so a download started elsewhere shows up too).
 */
@Composable
fun rememberDownloadMap(): Map<String, Download> {
    val core = LocalCore.current
    val account = LocalAccount.current

    fun read() = runCatching { core.downloads() }.getOrDefault(emptyList()).filter { it.accountId == account }.associateBy { it.itemId }
    var map by remember(account) { mutableStateOf(read()) }
    LaunchedEffect(account, DownloadPending.version) {
        map = read()
        while (true) {
            delay(if (map.values.any { !it.done }) 1000 else 3000)
            map = read()
        }
    }
    return map
}

/** "42%" while downloading, "0.8 GB" once done. */
fun Download.progressLabel(): String =
    if (done) {
        gb(total ?: downloaded)
    } else {
        val t = total?.toLong() ?: 0L
        if (t > 0) "${(downloaded.toLong() * 100 / t).coerceIn(0, 100)}%" else "…"
    }

/** Determinate ring (download progress). */
@Composable
fun ProgressRing(
    fraction: Float,
    size: androidx.compose.ui.unit.Dp = 40.dp,
) {
    Box(contentAlignment = Alignment.Center) {
        androidx.compose.material3.CircularProgressIndicator(
            progress = { fraction.coerceIn(0f, 1f) },
            modifier = Modifier.size(size),
            color = Accent,
            trackColor = Color.White.copy(alpha = 0.15f),
            strokeWidth = 3.dp,
        )
        Text("${(fraction * 100).toInt().coerceIn(0, 100)}", style = MaterialTheme.typography.labelMedium, color = Color.White)
    }
}

/**
 * The icon part of a download button: download arrow; a spinning ring while starting; a ring
 * filling up with the percentage while downloading; a filled download icon once downloaded
 * (on the accent background the button gives it).
 */
@Composable
fun DownloadStateIcon(
    dl: Download?,
    pending: Boolean = false,
    size: androidx.compose.ui.unit.Dp = 40.dp,
) {
    val t = dl?.total?.toLong() ?: 0L
    when {
        dl == null && !pending -> Icon(Icons.Download, "Download", Modifier.size(22.dp))
        dl == null || (!dl.done && (dl.downloaded == 0uL || t == 0L) && dl.state != DownloadState.PAUSED) ->
            androidx.compose.material3.CircularProgressIndicator(
                Modifier.size(size),
                color = Accent,
                trackColor = Color.White.copy(alpha = 0.15f),
                strokeWidth = 3.dp,
            )
        dl.done -> Icon(Icons.Download, "Downloaded", Modifier.size(22.dp), tint = Color.Black)
        dl.state == DownloadState.PAUSED || dl.state == DownloadState.PAUSED_NO_SPACE || dl.state == DownloadState.FAILED ->
            Box(contentAlignment = Alignment.Center) {
                ProgressRing(dl.downloaded.toLong().toFloat() / t.coerceAtLeast(1), size)
                Icon(Icons.Pause, "Paused", Modifier.size(16.dp), tint = Muted)
            }
        else -> ProgressRing(dl.downloaded.toLong().toFloat() / t, size)
    }
}

/** Round download button (episode rows) around [DownloadStateIcon]. */
@Composable
fun DownloadStateButton(
    dl: Download?,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    size: androidx.compose.ui.unit.Dp = 46.dp,
    pending: Boolean = false,
) {
    val label =
        when {
            dl == null -> if (pending) "Starting download" else "Download"
            dl.done -> "Downloaded, ${dl.progressLabel()}"
            else -> "Downloading, ${dl.progressLabel()}"
        }
    Box(
        modifier
            .size(size)
            .clip(androidx.compose.foundation.shape.CircleShape)
            .background(if (dl?.done == true) Accent else SurfaceHigh)
            .clickable(onClickLabel = label, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) { DownloadStateIcon(dl, pending, size - 6.dp) }
}

/** Next to the button: "Downloading 42% · 0.3 / 0.8 GB" or "Downloaded · 0.8 GB". */
fun Download.summary(): String =
    when {
        done -> "Downloaded · ${gb(total ?: downloaded)}"
        state == DownloadState.WAITING -> "Waiting for the network · ${progressLabel()}"
        state == DownloadState.PAUSED -> "Paused · ${progressLabel()}"
        state == DownloadState.FAILED -> "Download failed"
        state == DownloadState.PAUSED_NO_SPACE -> "Paused: storage full"
        else -> "Downloading ${progressLabel()}" + (total?.let { " · ${gb(downloaded)} / ${gb(it)}" } ?: "")
    }

private fun cached(
    core: AlwyWatch,
    account: String,
    id: String,
): Item? = runCatching { core.cachedItem(account, id)?.item }.getOrNull()

fun startDownloads(ctx: Context) {
    DownloadWorker.start(ctx)
}

// ----- Downloads mode (home organized like the main one) -----------------------------------

/** What is downloaded, organized like the home screen: in progress, then shows and movies. */
@Composable
fun DownloadsHome(modifier: Modifier = Modifier) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val (list, refresh) = rememberDownloads()
    val mine = list.filter { it.accountId == account }
    val active = mine.filter { !it.done }
    val done = mine.filter { it.done }
    val shows = done.filter { it.showKey() != null }.groupBy { it.showKey()!! }
    val movies = done.filter { it.showKey() == null }
    LazyColumn(modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 32.dp)) {
        if (mine.isEmpty()) {
            item {
                Column(
                    Modifier.fillMaxWidth().padding(top = 64.dp, start = d.gutter, end = d.gutter),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Icon(Icons.Download, null, Modifier.size(56.dp), tint = Subtle)
                    Text("Nothing downloaded yet", style = MaterialTheme.typography.titleMedium)
                    Text("Download a movie, an episode, a season or a whole show from its page.", color = Muted)
                }
            }
        }
        if (active.isNotEmpty()) {
            item { SectionHeader("Downloading", Modifier.padding(top = 16.dp, bottom = 8.dp)) }
            items(active, key = { it.id }) { DownloadRow(it, refresh, Modifier.padding(horizontal = d.gutter, vertical = 5.dp)) }
        }
        if (shows.isNotEmpty()) {
            item {
                SectionHeader("Shows", Modifier.padding(top = 24.dp))
                LazyRow(
                    contentPadding = PaddingValues(horizontal = d.gutter, vertical = 12.dp),
                    horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
                ) {
                    items(shows.entries.toList(), key = { it.key.first }) { (key, eps) ->
                        val (id, title) = key
                        val show = remember(id) { cached(core, account, id) }
                        val art = show?.poster() ?: eps.firstNotNullOfOrNull { cached(core, account, it.itemId)?.poster() }
                        PosterTile(id, art, title, "${eps.size} episode${if (eps.size > 1) "s" else ""}") {
                            nav.push(Route.DownloadedShow(id, title))
                        }
                    }
                }
            }
        }
        if (movies.isNotEmpty()) {
            item {
                SectionHeader("Movies", Modifier.padding(top = 16.dp))
                LazyRow(
                    contentPadding = PaddingValues(horizontal = d.gutter, vertical = 12.dp),
                    horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
                ) {
                    items(movies, key = { it.id }) { m ->
                        val item = remember(m.itemId) { cached(core, account, m.itemId) }
                        PosterTile(m.itemId, item?.poster(), m.title, m.status()) {
                            ctx.startActivity(PlayerActivity.intent(ctx, m.accountId, m.itemId, 0L))
                        }
                    }
                }
            }
        }
        if (mine.isNotEmpty()) {
            item {
                ActionButton(
                    "Manage downloads",
                    { nav.push(Route.ManageDownloads) },
                    Modifier.padding(horizontal = d.gutter, vertical = 24.dp),
                    icon = Icons.Settings,
                )
            }
        }
    }
}

@Composable
private fun PosterTile(
    id: String,
    image: be.alwyapps.watch.core.media.ImageRef?,
    title: String,
    subtitle: String,
    onClick: () -> Unit,
) {
    val d = LocalDimens.current
    Column(Modifier.width(d.posterWidth)) {
        FocusCard(onClick = onClick) {
            RemoteImage(id, image, d.posterWidth, Modifier.fillMaxWidth().aspectRatio(2f / 3f), contentDescription = title)
            Badge("Offline", Modifier.align(Alignment.TopStart).padding(8.dp), color = Color.Black.copy(alpha = 0.6f))
        }
        Text(title, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Text(subtitle, style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** One download with its state and actions (pause/resume, play, delete). */
@Composable
fun DownloadRow(
    dl: Download,
    refresh: () -> Unit,
    modifier: Modifier = Modifier,
    label: String = dl.title,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val item = remember(dl.itemId) { cached(core, dl.accountId, dl.itemId) }
    Row(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(d.radius + 4.dp))
            .background(SurfaceColor)
            .then(if (dl.done) Modifier.clickable { ctx.startActivity(PlayerActivity.intent(ctx, dl.accountId, dl.itemId, 0L)) } else Modifier)
            .padding(10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(Modifier.width(112.dp).clip(RoundedCornerShape(10.dp))) {
            RemoteImage(dl.itemId, item?.thumb(), 240.dp, Modifier.fillMaxWidth().aspectRatio(16f / 9f))
            if (dl.watchedAt != null) {
                Icon(
                    Icons.Check,
                    "Watched",
                    Modifier
                        .align(Alignment.TopEnd)
                        .padding(5.dp)
                        .size(18.dp)
                        .clip(RoundedCornerShape(50))
                        .background(Color.Black.copy(alpha = 0.6f))
                        .padding(3.dp),
                    tint = Accent,
                )
            }
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            // "Show · S2 · E6 · Title": the show, then the episode on its own line.
            val parts = label.split(" · ")
            if (parts.size >= 3) {
                Text(parts.first(), style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(parts.drop(1).joinToString(" · "), style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            } else {
                Text(label, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            Text(
                dl.status(),
                color = if (dl.state == DownloadState.FAILED || dl.state == DownloadState.PAUSED_NO_SPACE) Danger else Muted,
                style = MaterialTheme.typography.bodySmall,
                maxLines = 2,
            )
            val total = dl.total?.toLong() ?: 0L
            if (!dl.done && total > 0) {
                LinearProgressIndicator(
                    progress = { (dl.downloaded.toLong().toFloat() / total).coerceIn(0f, 1f) },
                    modifier =
                        Modifier
                            .fillMaxWidth()
                            .padding(top = 4.dp)
                            .height(4.dp)
                            .clip(RoundedCornerShape(50)),
                    color = Accent,
                    trackColor = SurfaceHigh,
                )
            }
        }
        when (dl.state) {
            DownloadState.COMPLETED -> {}
            DownloadState.RUNNING, DownloadState.QUEUED, DownloadState.WAITING ->
                IconAction(Icons.Pause, "Pause", {
                    core.pauseDownload(dl.id)
                    refresh()
                })
            else ->
                IconAction(Icons.Refresh, "Resume", {
                    core.resumeDownload(dl.id)
                    startDownloads(ctx)
                    refresh()
                })
        }
        var confirm by remember { mutableStateOf(false) }
        if (confirm) {
            be.alwyapps.watch.ui.ConfirmDialog(
                if (dl.done) "Delete the download?" else "Cancel the download?",
                "${dl.title.split(" · ").first()} (${dl.progressLabel()}) will be removed from this phone.",
                confirm = if (dl.done) "Delete" else "Cancel download",
                onConfirm = {
                    core.removeDownload(dl.id)
                    DownloadPending.changed()
                    refresh()
                },
                onDismiss = { confirm = false },
            )
        }
        IconAction(Icons.Delete, "Delete", { confirm = true })
    }
}

// ----- A downloaded show ----------------------------------------------------------------------

/** The downloaded episodes of one show, by season, plus its "keep next episodes" setting. */
@Composable
fun DownloadedShowScreen(
    seriesId: String,
    title: String,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    val (list, refresh) = rememberDownloads()
    val eps =
        list
            .filter { it.accountId == account }
            .filter { dl ->
                // Older downloads have no show metadata: their cached episode or title tell.
                dl.showKey()?.first == seriesId ||
                    dl.showKey()?.second == title ||
                    (dl.episode == null && cached(core, account, dl.itemId)?.seriesId == seriesId)
            }.sortedWith(compareBy({ it.seasonEpisode().first }, { it.seasonEpisode().second }))
    var auto by remember { mutableStateOf(core.autoDownloadOf(account, seriesId)) }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        item {
            be.alwyapps.watch.ui
                .PageTitle(title, "${eps.size} downloaded")
        }
        auto?.let { a ->
            item {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(d.radius + 4.dp))
                        .background(SurfaceColor)
                        .padding(14.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Column(Modifier.weight(1f)) {
                        Text("Keeps the next ${a.count} episodes", style = MaterialTheme.typography.titleSmall)
                        Text("Deletes them ${a.rule.label().lowercase()}", style = MaterialTheme.typography.bodySmall, color = Muted)
                    }
                    ActionButton("Stop", {
                        scope.launch {
                            runCatching { core.setAutoDownload(account, seriesId, title, 0u, DeleteRule.Never) }
                            auto = null
                        }
                    })
                }
            }
        }
        if (!seriesId.startsWith("title:")) {
            item {
                ActionButton("Open the show", {
                    nav.push(Route.Detail(seriesId))
                }, icon = Icons.Info)
            }
        }
        var lastSeason = -1
        for (dl in eps) {
            val (season, episode) = dl.seasonEpisode()
            if (season != lastSeason) {
                lastSeason = season
                item(key = "season-$season") {
                    Text(
                        if (season == 0) "Episodes" else "Season $season",
                        Modifier.padding(top = 14.dp, bottom = 4.dp),
                        style = MaterialTheme.typography.titleMedium,
                    )
                }
            }
            item(key = dl.id) { DownloadRow(dl, refresh, label = "E$episode · ${dl.episodeTitle()}") }
        }
        item { Spacer(Modifier.navigationBarsPadding().height(24.dp)) }
    }
}

// ----- Download sheet -------------------------------------------------------------------------

/**
 * Download choices for a show, a season or an episode: this one, its season, the whole show,
 * or "keep the next N episodes"; and when they delete themselves.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun DownloadSheet(
    series: Item,
    season: Item?,
    episode: Item?,
    onDismiss: () -> Unit,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val seriesId = series.id
    val seriesTitle = series.title
    val existing = remember { core.autoDownloadOf(account, seriesId) }
    var choice by remember {
        mutableStateOf(
            if (episode != null) {
                "episode"
            } else if (season != null) {
                "season"
            } else {
                "next"
            },
        )
    }
    var count by remember { mutableIntStateOf(existing?.count?.toInt() ?: 3) }
    var rule by remember { mutableStateOf(existing?.rule ?: if (choice == "next") DeleteRule.WhenWatched else core.defaultDeleteRule()) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val permission =
        androidx.activity.compose.rememberLauncherForActivityResult(
            androidx.activity.result.contract.ActivityResultContracts
                .RequestPermission(),
        ) { }
    ModalBottomSheet(onDismissRequest = onDismiss, containerColor = be.alwyapps.watch.ui.Background) {
        Column(
            Modifier.padding(horizontal = 20.dp).padding(bottom = 28.dp).navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Text("Download", style = MaterialTheme.typography.headlineMedium)
            Text(seriesTitle, color = Muted)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (episode != null) {
                    ActionButton("This episode", { choice = "episode" }, primary = choice == "episode")
                }
                if (season != null) {
                    ActionButton(season.title, { choice = "season" }, primary = choice == "season")
                }
                ActionButton("Whole show", { choice = "show" }, primary = choice == "show")
                ActionButton("Next episodes", {
                    choice = "next"
                    if (rule is DeleteRule.Never) rule = DeleteRule.WhenWatched
                }, primary = choice == "next")
            }
            if (choice == "next") {
                Text(
                    "Keeps your next unwatched episodes on this phone and replaces them as you watch.",
                    style = MaterialTheme.typography.bodySmall,
                    color = Muted,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (n in listOf(1, 3, 5, 10)) ActionButton("$n", { count = n }, primary = count == n)
                }
            }
            Text("Delete them", Modifier.padding(top = 4.dp), style = MaterialTheme.typography.titleMedium)
            RuleChips(rule) { rule = it }
            error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                ActionButton(if (busy) "Adding…" else "Download", {
                    if (busy) return@ActionButton
                    busy = true
                    if (android.os.Build.VERSION.SDK_INT >= 33) permission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                    // The page's button spins right away.
                    val marker = if (choice == "episode") episode!!.id else seriesId
                    DownloadPending.add(marker)
                    onDismiss()
                    // App-wide scope: the sheet (and its scope) is gone once dismissed.
                    ctx.services.scope.launch(kotlinx.coroutines.Dispatchers.Main) {
                        try {
                            when (choice) {
                                "episode" -> core.downloadItem(account, episode!!.id, rule)
                                "season" -> core.downloadSeason(account, seriesId, season!!.id, rule)
                                "show" -> core.downloadSeries(account, seriesId, rule)
                                else -> core.setAutoDownload(account, seriesId, seriesTitle, count.toUInt(), rule)
                            }
                            startDownloads(ctx)
                        } catch (e: CoreException) {
                            android.widget.Toast
                                .makeText(ctx, e.message, android.widget.Toast.LENGTH_LONG)
                                .show()
                        } finally {
                            busy = false
                            DownloadPending.remove(marker)
                        }
                    }
                }, primary = true, icon = Icons.Download)
                val all = remember { runCatching { core.downloads() }.getOrDefault(emptyList()).filter { it.accountId == account } }
                // Deleting asks first: (question, what to delete).
                var ask by remember { mutableStateOf<Pair<String, List<Download>>?>(null) }
                ask?.let { (question, list) ->
                    be.alwyapps.watch.ui.ConfirmDialog(
                        question,
                        "%.1f GB will be freed on this phone.".format(list.sumOf { it.downloaded.toLong() } / 1e9),
                        onConfirm = {
                            list.forEach { core.removeDownload(it.id) }
                            DownloadPending.changed()
                            onDismiss()
                        },
                        onDismiss = { ask = null },
                    )
                }
                val mine = episode?.let { ep -> all.firstOrNull { it.itemId == ep.id } }
                if (mine != null) {
                    ActionButton(if (mine.done) "Remove this episode" else "Cancel this episode", {
                        ask = (if (mine.done) "Remove this episode's download?" else "Cancel this episode's download?") to listOf(mine)
                    }, icon = Icons.Delete)
                }
                // The whole show: cancel what is still downloading, or remove everything.
                val show = all.filter { it.episode?.seriesId == seriesId }
                val running = show.filter { !it.done }
                if (running.isNotEmpty()) {
                    ActionButton("Cancel ${running.size} downloading", {
                        ask = "Cancel ${running.size} downloads in progress?" to running
                    }, icon = Icons.Close)
                }
                if (show.any { it.done }) {
                    ActionButton("Remove all ${show.size} downloads", {
                        ask = "Remove all ${show.size} downloads of $seriesTitle?" to show
                    }, icon = Icons.Delete)
                }
                if (existing != null) {
                    ActionButton("Stop keeping next", {
                        scope.launch {
                            runCatching { core.setAutoDownload(account, seriesId, seriesTitle, 0u, DeleteRule.Never) }
                            onDismiss()
                        }
                    })
                }
            }
        }
    }
}

/** The show an item belongs to, for the sheet (an episode's show may only be known by id). */
fun Item.showFor(): Item? =
    when (kind) {
        ItemKind.SERIES -> this
        ItemKind.EPISODE ->
            seriesId?.let { sid ->
                // The show's poster becomes its primary image (the episode's is a still).
                val posters =
                    images
                        .filter { it.kind == be.alwyapps.watch.core.media.ImageKind.SERIES_PRIMARY }
                        .map { it.copy(kind = be.alwyapps.watch.core.media.ImageKind.PRIMARY) }
                copy(
                    id = sid,
                    kind = ItemKind.SERIES,
                    title = seriesTitle ?: title,
                    images =
                        posters + images.filter { it.kind == be.alwyapps.watch.core.media.ImageKind.BACKDROP },
                )
            }
        else -> null
    }
