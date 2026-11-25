// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.graphics.BitmapFactory
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.focusable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.DashServer
import be.alwyapps.watch.core.keep.Live
import be.alwyapps.watch.core.keep.Sample
import be.alwyapps.watch.core.keep.Stats
import be.alwyapps.watch.core.keep.Stream
import be.alwyapps.watch.core.keep.StreamTrack
import be.alwyapps.watch.core.keep.TopTitle
import be.alwyapps.watch.core.keep.TopUser
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.OnSurface
import be.alwyapps.watch.ui.Outline
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.Success
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.SurfaceHigh
import be.alwyapps.watch.ui.dpadScroll
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import java.util.TimeZone

private val LocalColor = Color(0xFFFFB224)
private val RemoteColor = Color(0xFF5B9CFF)
private val MoviesColor = Color(0xFF3EC7B0)
private val ShowsColor = Color(0xFFE8689F)
private val DirectStreamColor = Color(0xFF7CC4FF)

/** Posters and avatars of the dashboard, decoded once per screen visit set. */
private object DashImages {
    val memory = mutableStateMapOf<String, ImageBitmap?>()
}

/**
 * Admins: what plays now on Plex and Jellyfin, and for each track whether it plays as is or what
 * the server converts it to; then bandwidth, CPU and RAM over the last minutes and who watched
 * what over the last days. One server or all merged (the web app's /dashboard).
 */
@Composable
fun DashboardScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    var server by rememberSaveable { mutableStateOf(core.getSetting("dashboard.server") ?: "all") }
    var days by rememberSaveable { mutableStateOf(30) }
    var live by remember { mutableStateOf<Live?>(null) }
    var liveError by remember { mutableStateOf<String?>(null) }
    var stats by remember { mutableStateOf<Stats?>(null) }
    var statsError by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(server) {
        live = null
        while (true) {
            try {
                live = core.keepAdminDashboardLive(server)
                liveError = null
            } catch (e: CoreException) {
                liveError = e.message
            }
            delay(5_000)
        }
    }
    LaunchedEffect(server, days) {
        stats = null
        statsError = null
        val tz = TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 60_000
        try {
            stats = core.keepAdminDashboardStats(server, days.toUInt(), tz)
        } catch (e: CoreException) {
            statsError = e.message
        }
    }
    val servers = live?.servers.orEmpty()
    val list = rememberLazyListState()
    LazyColumn(
        Modifier.fillMaxSize().dpadScroll(list),
        state = list,
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        item { PageTitle(tr("Dashboard")) }
        item {
            LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                item {
                    ActionButton(tr("All servers"), {
                        server = "all"
                        core.setSetting("dashboard.server", "all")
                    }, selected = server == "all")
                }
                items(servers, key = { it.id }) { s ->
                    ServerChip(s, server == s.id) {
                        server = s.id
                        core.setSetting("dashboard.server", s.id)
                    }
                }
            }
        }
        servers
            .filter { it.problem != null && (server == "all" || server == it.id) }
            .forEach { s ->
                item(key = "problem-${s.id}") { Text("${s.name}: ${problemText(s.problem!!)}", color = Accent) }
            }
        liveError?.let { item { Text(it, color = Accent) } }
        val l = live
        if (l == null && liveError == null) item { Text(tr("Loading…"), color = Muted) }
        if (l != null) {
            item(key = "now") { NowPlaying(l, merged = server == "all") }
            item(key = "bandwidth") {
                TimeChart(
                    tr("Bandwidth"),
                    l.samples,
                    listOf(
                        Series(tr("Remote"), RemoteColor) { it.remoteKbps.toDouble() },
                        Series(tr("Local"), LocalColor) { it.localKbps.toDouble() },
                    ),
                    max = null,
                    format = { mbps(it.toLong()) },
                )
            }
            item(key = "cpu") {
                TimeChart(tr("CPU"), l.samples, listOf(Series(tr("Server"), LocalColor) { it.cpu?.toDouble() }), max = 100.0, format = ::pct)
            }
            item(key = "ram") {
                val gb = l.ramTotalMb.toLong() / 1024.0
                val title = if (gb > 0) "${tr("RAM")} · ${"%.0f".format(gb)} GB" else tr("RAM")
                TimeChart(title, l.samples, listOf(Series(tr("Server"), RemoteColor) { it.ram?.toDouble() }), max = 100.0, format = ::pct)
            }
        }
        item(key = "stats-head") {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(tr("Over the last days"), style = MaterialTheme.typography.titleLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf(7, 30, 90).forEach { n ->
                        ActionButton(trn("{n} day", "{n} days", n), { days = n }, selected = days == n)
                    }
                }
            }
        }
        statsError?.let { item { Text(it, color = Accent) } }
        val st = stats
        if (st == null && statsError == null) item { Text(tr("Loading…"), color = Muted) }
        if (st != null) {
            if (st.since.isNotEmpty()) {
                item {
                    val names = servers.associate { it.id to it.name }
                    Text(
                        st.since.joinToString(" · ") {
                            val date =
                                java.text.DateFormat
                                    .getDateInstance()
                                    .format(java.util.Date(it.since * 1000))
                            tr("{server}: history since {date}", "server" to (names[it.serverId] ?: ""), "date" to date)
                        },
                        color = Muted,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
            item(key = "users") { Panel(tr("Top users")) { TopUsers(st.topUsers) } }
            item(key = "history") {
                val decided = st.directPlays + st.transcodes
                Panel(
                    tr("Play history"),
                    if (decided > 0u) {
                        tr("{direct} direct play · {transcodes} converted (Plex)", "direct" to st.directPlays, "transcodes" to st.transcodes)
                    } else {
                        null
                    },
                ) { History(st) }
            }
            item(key = "top") {
                Panel(tr("Top played")) {
                    TitleList(tr("Movies"), st.topMovies)
                    Spacer(Modifier.height(12.dp))
                    TitleList(tr("TV"), st.topShows)
                }
            }
        }
    }
}

private fun problemText(p: String): String =
    when (p) {
        "no_tautulli" -> tr("Plex is read through Tautulli: set it up in Server settings.")
        "no_key" -> tr("Needs this server's API key in Server settings.")
        "unreachable" -> tr("Unreachable right now.")
        else -> p
    }

private fun mbps(kbps: Long): String = if (kbps >= 1000) "%.1f Mbps".format(kbps / 1000.0) else "$kbps kbps"

private fun pct(v: Double): String = "%.0f %%".format(v)

private fun clock(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    val h = s / 3600
    val m = s / 60 % 60
    val sec = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}

private fun hours(seconds: Long): String {
    val h = seconds / 3600
    val m = seconds / 60 % 60
    return when {
        h == 0L -> "$m min"
        m == 0L -> "$h h"
        else -> "%d h %02d".format(h, m)
    }
}

private fun decisionName(d: String): String =
    when (d) {
        "transcode" -> tr("Transcode")
        "direct_stream" -> tr("Direct stream")
        else -> tr("Direct play")
    }

private fun converted(s: Stream) = listOfNotNull(s.video, s.audio, s.subtitleTrack).any { it.decision == "transcode" }

/** The mark of a server type, drawn like [be.alwyapps.watch.ui.ServerBadge]. */
@Composable
fun ServerMark(
    kind: String,
    size: Dp = 22.dp,
    modifier: Modifier = Modifier,
) {
    val plex = kind == "plex"
    Canvas(modifier.size(size).clip(CircleShape).background(Color.Black.copy(alpha = 0.72f))) {
        val inset = this.size.width * 0.23f
        val w = this.size.width - 2 * inset
        val h = this.size.height - 2 * inset

        fun p(
            x: Float,
            y: Float,
        ) = Offset(inset + w * x, inset + h * y)
        if (plex) {
            val path =
                Path().apply {
                    listOf(p(0.22f, 0f), p(0.52f, 0f), p(0.86f, 0.5f), p(0.52f, 1f), p(0.22f, 1f), p(0.56f, 0.5f)).forEachIndexed { i, o ->
                        if (i == 0) moveTo(o.x, o.y) else lineTo(o.x, o.y)
                    }
                    close()
                }
            drawPath(path, Color(0xFFE5A00D))
        } else {
            val outer =
                Path().apply {
                    moveTo(p(0.5f, 0f).x, p(0.5f, 0f).y)
                    lineTo(p(1f, 1f).x, p(1f, 1f).y)
                    lineTo(p(0f, 1f).x, p(0f, 1f).y)
                    close()
                }
            val inner =
                Path().apply {
                    moveTo(p(0.5f, 0.42f).x, p(0.5f, 0.42f).y)
                    lineTo(p(0.72f, 0.86f).x, p(0.72f, 0.86f).y)
                    lineTo(p(0.28f, 0.86f).x, p(0.28f, 0.86f).y)
                    close()
                }
            drawPath(outer, Brush.linearGradient(listOf(Color(0xFFAA5CC3), Color(0xFF00A4DC)), p(0f, 0f), p(1f, 1f)))
            drawPath(inner, Color.Black.copy(alpha = 0.72f))
        }
    }
}

@Composable
private fun ServerChip(
    s: DashServer,
    selected: Boolean,
    onClick: () -> Unit,
) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        ServerMark(s.kind, 22.dp)
        ActionButton(s.name, onClick, selected = selected)
    }
}

/** A poster or avatar through the AlwyWatch server. */
@Composable
private fun DashImage(
    server: String,
    img: String?,
    modifier: Modifier,
) {
    val core = LocalCore.current
    val key = "$server|$img"
    val bitmap = DashImages.memory[key]
    // The effect stays in the composition: writing "started" into the map must not cancel it.
    LaunchedEffect(key) {
        if (img != null && key !in DashImages.memory) {
            DashImages.memory[key] = null
            val bytes = runCatching { core.keepAdminDashboardImage(server, img) }.getOrNull()
            DashImages.memory[key] =
                bytes?.let {
                    withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(it, 0, it.size)?.asImageBitmap() }
                }
        }
    }
    Box(modifier.background(SurfaceHigh)) {
        bitmap?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
    }
}

@Composable
private fun Panel(
    title: String,
    note: String? = null,
    content: @Composable () -> Unit,
) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .background(SurfaceColor)
            .border(1.dp, Outline, RoundedCornerShape(16.dp))
            .padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(title.uppercase(), style = MaterialTheme.typography.labelLarge, color = OnSurface)
        note?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = Muted) }
        content()
    }
}

@Composable
private fun NowPlaying(
    l: Live,
    merged: Boolean,
) {
    val n = l.streams.size
    val conv = l.streams.count(::converted)
    val total = l.streams.sumOf { it.kbps.toLong() }
    val summary =
        if (n == 0) {
            tr("Nothing is playing.")
        } else {
            listOf(
                trn("{n} stream", "{n} streams", n),
                if (conv == 0) tr("all direct") else trn("{n} converted", "{n} converted", conv),
                mbps(total),
            ).joinToString(" · ")
        }
    val d = LocalDimens.current
    Panel(tr("Now playing"), summary) {
        BoxWithConstraints {
            // Phones: two compact cards per row; TV and tablets: wide cards.
            val columns =
                if (d.isTv || maxWidth >= 600.dp) (maxWidth / 320.dp).toInt().coerceIn(1, 4) else 2
            val cardWidth = (maxWidth - 12.dp * (columns - 1)) / columns
            FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                l.streams.forEach { s ->
                    val server = l.servers.firstOrNull { it.id == s.serverId }
                    StreamCard(s, server?.kind ?: "", server?.name?.takeIf { merged }, cardWidth < 260.dp, Modifier.width(cardWidth))
                }
            }
        }
    }
}

@Composable
private fun StreamCard(
    s: Stream,
    kind: String,
    serverName: String?,
    compact: Boolean,
    modifier: Modifier,
) {
    val d = LocalDimens.current
    val pad = if (compact) 8.dp else 10.dp
    val small = MaterialTheme.typography.bodySmall
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val edge = if (converted(s)) Accent else Success
    Column(
        modifier
            .clip(RoundedCornerShape(12.dp))
            .background(SurfaceColor)
            // TV: each card takes the focus, so the remote walks through the streams.
            .then(if (d.isTv) Modifier.focusable(interactionSource = interaction) else Modifier)
            .border(if (focused) 2.dp else 0.dp, if (focused) Color.White else Color.Transparent, RoundedCornerShape(12.dp)),
    ) {
        Box(Modifier.fillMaxWidth().height(3.dp).background(edge))
        Row(Modifier.padding(pad), horizontalArrangement = Arrangement.spacedBy(if (compact) 8.dp else 12.dp)) {
            Box {
                DashImage(
                    s.serverId,
                    s.image,
                    Modifier.width(if (compact) 44.dp else 64.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(6.dp)),
                )
                ServerMark(kind, if (compact) 16.dp else 22.dp, Modifier.padding(if (compact) 2.dp else 4.dp))
            }
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(
                    s.title,
                    fontWeight = FontWeight.SemiBold,
                    style = if (compact) MaterialTheme.typography.bodyMedium else LocalTextStyle.current,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
                if (s.subtitle.isNotEmpty()) {
                    Text(s.subtitle, color = Muted, style = small, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
                Text(clock(s.durationMs), color = Muted, style = small)
                serverName?.let { Text(it, color = Subtle, style = small, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            }
        }
        val pct = if (s.durationMs > 0) (s.positionMs.toFloat() / s.durationMs).coerceIn(0f, 1f) else 0f
        Box(Modifier.fillMaxWidth().height(3.dp).background(SurfaceHigh)) {
            Box(Modifier.fillMaxWidth(pct).height(3.dp).background(Accent))
        }
        Column(Modifier.padding(pad), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(
                "${s.product} — ${s.device}",
                fontWeight = FontWeight.SemiBold,
                style = if (compact) small else MaterialTheme.typography.bodyMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
            val state =
                when (s.state) {
                    "paused" -> tr("Paused")
                    "buffering" -> tr("Buffering")
                    else -> tr("Playing")
                }
            Text("$state — ${clock(s.positionMs)} / ${clock(s.durationMs)}", color = Muted, style = MaterialTheme.typography.bodySmall)
            val where = if (s.local) tr("Local") else tr("Remote")
            val ip = if (s.ip.isNotEmpty()) " (${s.ip})" else ""
            val rate = if (s.kbps > 0u) " — ${mbps(s.kbps.toLong())}" else ""
            Text("$where$ip$rate", color = Muted, style = small)
            Spacer(Modifier.height(if (compact) 2.dp else 4.dp))
            TrackRow(tr("Video"), s.video, compact)
            TrackRow(tr("Audio"), s.audio, compact)
            TrackRow(tr("Subtitles"), s.subtitleTrack, compact)
        }
        Row(
            Modifier.fillMaxWidth().background(Color.Black.copy(alpha = 0.15f)).padding(pad),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(if (compact) 8.dp else 10.dp),
        ) {
            Avatar(s.serverId, s.userAvatar, s.user, if (compact) 24.dp else 32.dp)
            Text(s.user, style = if (compact) small else LocalTextStyle.current, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun Avatar(
    server: String,
    img: String?,
    name: String,
    size: Dp,
) {
    if (img != null) {
        DashImage(server, img, Modifier.size(size).clip(CircleShape))
    } else {
        Box(Modifier.size(size).clip(CircleShape).background(SurfaceHigh), contentAlignment = Alignment.Center) {
            Text(name.take(1).uppercase(), fontWeight = FontWeight.SemiBold)
        }
    }
}

/** What the file has, then whether it plays as is or what it becomes. */
@Composable
private fun TrackRow(
    kind: String,
    t: StreamTrack?,
    compact: Boolean,
) {
    t ?: return
    val (color, sign) =
        when (t.decision) {
            "transcode" -> Accent to "⚙ "
            "direct_stream" -> DirectStreamColor to "⇄ "
            else -> Success to "✓ "
        }
    val verdict = sign + decisionName(t.decision) + (t.to?.let { " → $it" } ?: "")
    val weight = if (t.decision == "transcode") FontWeight.SemiBold else FontWeight.Normal
    if (compact) {
        // Narrow card: the kind above, no label column.
        Column(Modifier.fillMaxWidth().padding(vertical = 2.dp)) {
            Box(Modifier.fillMaxWidth().height(1.dp).background(Outline))
            Text(kind.uppercase(), color = Subtle, style = MaterialTheme.typography.labelSmall, modifier = Modifier.padding(top = 4.dp))
            Text(t.label, style = MaterialTheme.typography.bodySmall)
            Text(verdict, color = color, style = MaterialTheme.typography.bodySmall, fontWeight = weight)
        }
        return
    }
    Column(Modifier.fillMaxWidth().padding(vertical = 4.dp)) {
        Box(Modifier.fillMaxWidth().height(1.dp).background(Outline))
        Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(kind, color = Muted, style = MaterialTheme.typography.bodySmall, modifier = Modifier.width(70.dp))
            Column {
                Text(t.label, style = MaterialTheme.typography.bodyMedium)
                Text(verdict, color = color, style = MaterialTheme.typography.bodySmall, fontWeight = weight)
            }
        }
    }
}

private class Series(
    val name: String,
    val color: Color,
    val value: (Sample) -> Double?,
)

/** A line chart of one or two series over the last minutes; the latest values above it. */
@Composable
private fun TimeChart(
    title: String,
    samples: List<Sample>,
    series: List<Series>,
    max: Double?,
    format: (Double) -> String,
) {
    val top =
        max ?: (samples.flatMap { s -> series.mapNotNull { it.value(s) } }.maxOrNull() ?: 0.0)
            .let { if (it <= 0) 1.0 else it * 1.15 }
    val span =
        if (samples.size > 1) {
            tr("last {n} min", "n" to Math.round((samples.last().t - samples.first().t) / 60.0))
        } else {
            null
        }
    Panel(title, span) {
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            series.forEach { s ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(10.dp).clip(CircleShape).background(s.color))
                    Spacer(Modifier.width(6.dp))
                    val v = samples.lastOrNull()?.let(s.value)
                    Text("${s.name} ${v?.let(format) ?: "—"}", color = Muted, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        Box(Modifier.fillMaxWidth().height(120.dp)) {
            Canvas(Modifier.fillMaxSize()) {
                val w = size.width
                val h = size.height
                drawLine(Outline, Offset(0f, h / 2), Offset(w, h / 2), pathEffect = PathEffect.dashPathEffect(floatArrayOf(8f, 10f)))
                drawLine(SurfaceHigh, Offset(0f, h), Offset(w, h))
                val n = samples.size
                series.forEach { s ->
                    val path = Path()
                    var first = true
                    samples.forEachIndexed { i, sample ->
                        val v = s.value(sample)
                        if (v == null) {
                            first = true
                        } else {
                            val x = if (n <= 1) w else i * w / (n - 1)
                            val y = h - (v / top).coerceAtMost(1.0).toFloat() * h
                            if (first) path.moveTo(x, y) else path.lineTo(x, y)
                            first = false
                        }
                    }
                    drawPath(path, s.color, style = Stroke(width = 2.dp.toPx(), join = StrokeJoin.Round))
                }
            }
            Text(format(top), color = Subtle, style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun TopUsers(users: List<TopUser>) {
    if (users.isEmpty()) {
        Text(tr("Nothing yet."), color = Muted)
        return
    }
    LazyRow(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        items(users) { u ->
            Column(
                Modifier
                    .width(200.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(SurfaceColor)
                    .padding(12.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Avatar(u.serverId, u.avatar, u.name, 44.dp)
                    Column {
                        Text(u.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(trn("{n} play", "{n} plays", u.plays.toInt()), color = Muted, style = MaterialTheme.typography.bodySmall)
                        Text(hours(u.seconds.toLong()), color = Muted, style = MaterialTheme.typography.bodySmall)
                    }
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(tr("TV"), style = MaterialTheme.typography.bodySmall)
                    Text(hours(u.episodeSeconds.toLong()), style = MaterialTheme.typography.bodySmall)
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                    Text(tr("Movies"), style = MaterialTheme.typography.bodySmall)
                    Text(hours(u.movieSeconds.toLong()), style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

/** Plays per day, episodes and movies stacked. */
@Composable
private fun History(st: Stats) {
    val days = st.history
    val top = (days.maxOfOrNull { (it.movies + it.episodes).toInt() } ?: 0).coerceAtLeast(1) * 1.1f
    Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
        listOf(tr("TV") to (ShowsColor to days.sumOf { it.episodes.toInt() }), tr("Movies") to (MoviesColor to days.sumOf { it.movies.toInt() }))
            .forEach { (name, v) ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(10.dp).clip(CircleShape).background(v.first))
                    Spacer(Modifier.width(6.dp))
                    Text("$name ${v.second}", color = Muted, style = MaterialTheme.typography.bodySmall)
                }
            }
    }
    Canvas(Modifier.fillMaxWidth().height(140.dp)) {
        val n = days.size.coerceAtLeast(1)
        val slot = size.width / n
        val gap = if (slot > 6.dp.toPx()) 2.dp.toPx() else 0f
        days.forEachIndexed { i, day ->
            val x = i * slot + gap / 2
            val bw = slot - gap
            val eh = day.episodes.toFloat() / top * size.height
            val mh = day.movies.toFloat() / top * size.height
            drawRect(ShowsColor, Offset(x, size.height - eh), Size(bw, eh))
            drawRect(MoviesColor, Offset(x, size.height - eh - mh), Size(bw, mh))
        }
    }
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(days.firstOrNull()?.date ?: "", color = Subtle, style = MaterialTheme.typography.labelSmall)
        Text(days.lastOrNull()?.date ?: "", color = Subtle, style = MaterialTheme.typography.labelSmall)
    }
}

@Composable
private fun TitleList(
    title: String,
    list: List<TopTitle>,
) {
    Text(title, color = Muted, style = MaterialTheme.typography.titleSmall)
    if (list.isEmpty()) {
        Text(tr("Nothing yet."), color = Muted, style = MaterialTheme.typography.bodySmall)
        return
    }
    LazyRow(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        items(list) { t ->
            Column(Modifier.width(110.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                DashImage(t.serverId, t.image, Modifier.fillMaxWidth().aspectRatio(2f / 3f).clip(RoundedCornerShape(8.dp)))
                Text(t.title, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, fontWeight = FontWeight.SemiBold)
                Text(trn("{n} play", "{n} plays", t.plays.toInt()), color = Muted, style = MaterialTheme.typography.labelSmall)
                Text(trn("{n} person", "{n} people", t.users.toInt()), color = Muted, style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}
