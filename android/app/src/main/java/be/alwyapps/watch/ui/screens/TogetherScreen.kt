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
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.KnockStatus
import be.alwyapps.watch.core.keep.TogetherMedia
import be.alwyapps.watch.core.keep.TogetherRoom
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.player.PlayerActivity
import be.alwyapps.watch.player.formatTime
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalItemOwners
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.RemoteImage
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.poster
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Watch together: the rooms open on the AlwyWatch server (everyone signed in to it sees them).
 * Joining asks the room's host, who lets you in from a popup in their player.
 */
@Composable
fun TogetherScreen() {
    val d = LocalDimens.current
    val nav = be.alwyapps.watch.ui.LocalNavigator.current
    val (rooms, unreachable) = rememberTogetherRooms(everyMs = 8_000)
    var joining by remember { mutableStateOf<TogetherRoom?>(null) }
    joining?.let { JoinDialog(it) { joining = null } }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item(key = "title") {
            Column(Modifier.widthIn(max = 640.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                PageTitle("Watch together")
                Text(
                    "What friends on your AlwyWatch server are watching together. Join one: its host lets you in.",
                    color = Muted,
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        val list = rooms.orEmpty()
        if (rooms != null && list.isEmpty()) {
            item(key = "empty") {
                Text(
                    if (unreachable) {
                        "Can't reach your AlwyWatch server right now. Trying again…"
                    } else {
                        "Nobody is watching together right now. To start, open a movie or an episode and choose " +
                            "Watch together: friends will see it here."
                    },
                    Modifier.widthIn(max = 640.dp).padding(top = 12.dp),
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
            // Something to focus on an empty page (TV: the D-pad, and closing the menu, need it).
            item(key = "start") {
                ActionButton(
                    "Choose something to watch",
                    { nav.reset(be.alwyapps.watch.ui.Route.Home) },
                    Modifier.padding(top = 4.dp).initialFocus(),
                    primary = true,
                    icon = Icons.Play,
                )
            }
        }
        items(list, key = { it.code }) { room ->
            RoomRow(room, Modifier.widthIn(max = 760.dp).then(if (room == list.first()) Modifier.initialFocus() else Modifier)) {
                joining = room
            }
        }
    }
}

/** Home: the rooms open right now, as a row (nothing when there are none). */
@Composable
fun TogetherRail() {
    val d = LocalDimens.current
    val rooms = rememberTogetherRooms(everyMs = 30_000).first.orEmpty().filter { !it.mine }
    if (rooms.isEmpty()) return
    var joining by remember { mutableStateOf<TogetherRoom?>(null) }
    joining?.let { JoinDialog(it) { joining = null } }
    Column(Modifier.padding(top = if (d.isTv) 18.dp else 24.dp)) {
        be.alwyapps.watch.ui
            .SectionHeader("Watching together now", subtitle = "Join friends on your AlwyWatch server")
        LazyRow(
            contentPadding = PaddingValues(horizontal = d.gutter, vertical = if (d.isTv) 14.dp else 12.dp),
            horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        ) {
            items(rooms, key = { it.code }) { room ->
                RoomRow(room, Modifier.width(if (d.isTv) 460.dp else 320.dp)) { joining = room }
            }
        }
    }
}

/**
 * Open rooms, refreshed every [everyMs] while shown (null until the first answer), and whether
 * the last try failed (the last known list stays).
 */
@Composable
private fun rememberTogetherRooms(everyMs: Long): Pair<List<TogetherRoom>?, Boolean> {
    val core = LocalCore.current
    val rooms by produceState<Pair<List<TogetherRoom>?, Boolean>>(null to false) {
        while (true) {
            value = runCatching { core.togetherRooms() }.fold({ it to false }, { (value.first ?: emptyList()) to true })
            delay(everyMs)
        }
    }
    return rooms
}

/** The room's title on this user's servers (each member plays their own copy). */
@Composable
private fun rememberRoomTitle(media: TogetherMedia): Pair<String, Item>? {
    val core = LocalCore.current
    val found by produceState<Pair<String, Item>?>(null, media) {
        value =
            runCatching {
                val accounts = core.accounts().map { it.id }
                val m = core.togetherFind(preferredFirst(core, accounts), media) ?: return@runCatching null
                val item = core.cachedItem(m.accountId, m.itemId)?.item ?: core.refreshItem(m.accountId, m.itemId, false).item
                m.accountId to item
            }.getOrNull()
    }
    return found
}

private fun TogetherMedia.label(): String = seriesTitle?.let { "$it · S${season ?: 1} · E${episode ?: 1}" } ?: title

@Composable
private fun RoomRow(
    room: TogetherRoom,
    modifier: Modifier = Modifier,
    onJoin: () -> Unit,
) {
    val d = LocalDimens.current
    val found = rememberRoomTitle(room.media)
    val posterW: Dp = if (d.isTv) 84.dp else 64.dp
    FocusCard(onClick = onJoin, modifier = modifier.fillMaxWidth(), glow = false, focusScale = 1.02f) {
        Row(
            Modifier.fillMaxWidth().background(SurfaceColor).padding(10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Box(Modifier.width(posterW).aspectRatio(2f / 3f)) {
                val (account, item) = found ?: (null to null)
                if (item != null && account != null) {
                    androidx.compose.runtime.CompositionLocalProvider(LocalItemOwners provides mapOf(item.id to account)) {
                        RemoteImage(item.id, item.poster(), posterW, Modifier.fillMaxSize())
                    }
                } else {
                    Box(Modifier.fillMaxSize().background(Background), contentAlignment = Alignment.Center) {
                        androidx.compose.material3.Icon(Icons.Group, null, tint = Muted)
                    }
                }
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(room.media.label(), style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                val others = room.members.size - 1
                Text(
                    if (others > 0) "${room.host} and $others other${if (others > 1) "s" else ""}" else room.host,
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                val now = room.positionMs.toLong() + if (room.playing) (room.nowMs - room.atMs).coerceAtLeast(0) else 0
                Text(
                    listOfNotNull(
                        if (room.playing) "Playing" else "Paused",
                        formatTime(now),
                        "you're in it on another device".takeIf { room.mine },
                    ).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = Muted,
                )
            }
            if (!d.isTv) ActionButton("Join", onJoin, primary = true)
        }
    }
}

private sealed interface Joining {
    data object Asking : Joining

    data class Ended(
        val message: String,
    ) : Joining
}

/**
 * Asks the host and waits for their answer; accepted, the player opens in sync with the room.
 * Closing the dialog while waiting withdraws the request.
 */
@Composable
private fun JoinDialog(
    room: TogetherRoom,
    onClose: () -> Unit,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    var state by remember { mutableStateOf<Joining>(Joining.Asking) }
    var request by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(room.code) {
        val accounts = runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id }
        val found = runCatching { core.togetherFind(preferredFirst(core, accounts), room.media) }.getOrNull()
        if (found == null) {
            state = Joining.Ended("${room.media.label()} isn't on your servers, so you can't watch it with them.")
            return@LaunchedEffect
        }
        try {
            var k = core.togetherKnock(room.code)
            request = k.requestId
            var failures = 0
            while (k.status == KnockStatus.WAITING) {
                k =
                    try {
                        core.togetherKnockWait(room.code, k.requestId).also { failures = 0 }
                    } catch (e: CoreException) {
                        if (e is CoreException.NotFound || ++failures > 5) throw e
                        delay(2000L * failures)
                        continue
                    }
            }
            request = null
            val joined = k.joined
            if (k.status == KnockStatus.DECLINED || joined == null) {
                state = Joining.Ended("${room.host} can't let you in right now.")
                return@LaunchedEffect
            }
            val receivedAt = android.os.SystemClock.elapsedRealtime()
            val start =
                be.alwyapps.watch.core
                    .togetherPosition(joined.state, receivedAt, android.os.SystemClock.elapsedRealtime())
                    .toLong()
            ctx.startActivity(PlayerActivity.intent(ctx, found.accountId, found.itemId, start, joined.state.code, joined.memberId))
            onClose()
        } catch (e: CoreException) {
            request = null
            state =
                Joining.Ended(
                    if (e is CoreException.NotFound) "This session has ended." else "Couldn't ask to join: ${e.message}",
                )
        }
    }
    // Leaving while waiting: no longer asking.
    DisposableEffect(room.code) {
        onDispose {
            val id = request ?: return@onDispose
            ctx.services.scope.launch { runCatching { core.togetherKnockCancel(room.code, id) } }
        }
    }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onClose,
        containerColor = Background,
        icon = { androidx.compose.material3.Icon(Icons.Group, null, tint = be.alwyapps.watch.ui.Accent) },
        title = { Text(room.media.label()) },
        text = {
            when (val s = state) {
                Joining.Asking ->
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        androidx.compose.material3.CircularProgressIndicator(Modifier.width(22.dp), strokeWidth = 2.dp)
                        Text("Waiting for ${room.host} to let you in…", style = MaterialTheme.typography.bodyLarge)
                    }
                is Joining.Ended -> Text(s.message, style = MaterialTheme.typography.bodyLarge)
            }
        },
        confirmButton = {
            // A dialog of its own: the screen's "focus once" is already used.
            androidx.compose.runtime.CompositionLocalProvider(be.alwyapps.watch.ui.LocalInitialFocusDone provides null) {
                ActionButton(if (state == Joining.Asking) "Cancel" else "OK", onClose, Modifier.initialFocus(), primary = state != Joining.Asking)
            }
        },
    )
}
