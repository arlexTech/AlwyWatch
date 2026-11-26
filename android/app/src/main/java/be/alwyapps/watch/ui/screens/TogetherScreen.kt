// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.TogetherMedia
import be.alwyapps.watch.core.keep.TogetherPerson
import be.alwyapps.watch.core.keep.TogetherRoom
import be.alwyapps.watch.core.keep.TogetherState
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.player.PlayerActivity
import be.alwyapps.watch.player.formatTime
import be.alwyapps.watch.services
import be.alwyapps.watch.together.TogetherInbox
import be.alwyapps.watch.together.TogetherSession
import be.alwyapps.watch.together.friendly
import be.alwyapps.watch.together.label
import be.alwyapps.watch.together.people
import be.alwyapps.watch.together.untitled
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Avatar
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.ConfirmDialog
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalInitialFocusDone
import be.alwyapps.watch.ui.LocalItemOwners
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.MinimalScrollSpec
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Navigator
import be.alwyapps.watch.ui.OnSurface
import be.alwyapps.watch.ui.Outline
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.RemoteImage
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.SectionHeader
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.focusOnAppear
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.poster
import be.alwyapps.watch.ui.rememberFocus
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import kotlinx.coroutines.launch

// ----- Opening rooms --------------------------------------------------------------------------

/**
 * "Watch together" on a title: opens a private waiting room with it and shows the Room screen
 * (never the player: the host starts when everyone is there). Hosting a room already: this
 * title becomes the room's (everyone stays in it).
 */
suspend fun createRoom(
    core: AlwyWatch,
    nav: Navigator,
    account: String,
    itemId: String,
    resumeMs: Long,
) {
    val mine = TogetherSession.seat
    if (mine != null && TogetherSession.isHost()) {
        val s = core.togetherUpdate(mine.code, mine.memberId, false, resumeMs.coerceAtLeast(0).toULong(), account, itemId)
        TogetherSession.accept(s)
        TogetherSession.startAt = resumeMs
        if (nav.current != Route.Room(mine.code)) nav.push(Route.Room(mine.code))
        return
    }
    val joined = core.togetherCreate(account, itemId)
    // One room at a time on a device.
    if (TogetherSession.seat?.code != joined.state.code) TogetherSession.seat?.let { leaveCurrentRoom() }
    TogetherSession.begin(core, joined, startAt = resumeMs)
    nav.push(Route.Room(joined.state.code))
}

/**
 * "New room" on the Together page: a waiting room with no title yet. People join while the host
 * browses; a title's "Watch in your room" then chooses what to watch.
 */
suspend fun openEmptyRoom(
    core: AlwyWatch,
    nav: Navigator,
) {
    val joined = core.togetherOpen()
    if (TogetherSession.seat?.code != joined.state.code) TogetherSession.seat?.let { leaveCurrentRoom() }
    TogetherSession.begin(core, joined)
    nav.push(Route.Room(joined.state.code))
}

/** Leaves the room this device sits in, for good. */
fun leaveCurrentRoom() = TogetherSession.leave()

/** An invitation accepted (or one of my rooms opened from another device): the Room screen. */
suspend fun joinRoom(
    core: AlwyWatch,
    nav: Navigator,
    code: String,
) {
    val joined = core.togetherJoin(code)
    if (TogetherSession.seat != null && TogetherSession.seat?.code != code) leaveCurrentRoom()
    TogetherSession.begin(core, joined)
    if (TogetherInbox.prompt?.code == code) TogetherInbox.prompt = null
    nav.push(Route.Room(code))
}

/** The player, in sync with the started room this device sits in. A problem to show, or null. */
suspend fun openRoomPlayer(
    ctx: Context,
    core: AlwyWatch,
): String? {
    val s = TogetherSession.state ?: return null
    val seat = TogetherSession.seat ?: return null
    TogetherSession.handledRound = maxOf(TogetherSession.handledRound, s.round)
    val accounts = runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id }
    val found =
        runCatching { core.togetherFind(preferredFirst(core, accounts), s.media) }.getOrNull()
            ?: return tr("{title} isn't on your servers, so you can't watch it with them.", "title" to s.media.label())
    val now = TogetherSession.state ?: s
    val start =
        be.alwyapps.watch.core
            .togetherPositionAt(now, core.togetherNowMs())
            .toLong()
    ctx.startActivity(PlayerActivity.intent(ctx, found.accountId, found.itemId, start, seat.code, seat.memberId))
    return null
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

@Composable
private fun RoomPoster(
    media: TogetherMedia,
    width: Dp,
) {
    val found = rememberRoomTitle(media)
    Box(Modifier.width(width).aspectRatio(2f / 3f).clip(RoundedCornerShape(8.dp))) {
        val (account, item) = found ?: (null to null)
        if (item != null && account != null) {
            CompositionLocalProvider(LocalItemOwners provides mapOf(item.id to account)) {
                RemoteImage(item.id, item.poster(), width, Modifier.fillMaxSize())
            }
        } else {
            Box(Modifier.fillMaxSize().background(SurfaceColor), contentAlignment = Alignment.Center) {
                Icon(Icons.Group, null, tint = Muted)
            }
        }
    }
}

// ----- Together page --------------------------------------------------------------------------

/** Watch together: the rooms I'm in and the ones I'm invited to (nobody sees other rooms). */
@Composable
fun TogetherScreen() {
    val d = LocalDimens.current
    val core = LocalCore.current
    val nav = LocalNavigator.current
    var loaded by remember { mutableStateOf(TogetherInbox.rooms != null) }
    LaunchedEffect(Unit) {
        TogetherInbox.refresh(core)
        loaded = true
    }
    val list = TogetherInbox.rooms?.rooms.orEmpty()
    val invites = list.filter { it.invited }
    val mine = list.filter { it.mine }
    val scope = rememberCoroutineScope()
    var opening by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 24.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item(key = "title") {
            Column(Modifier.widthIn(max = 640.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                PageTitle(tr("Watch together"))
                Text(
                    tr("Rooms are private: only the people you invite can see them and join."),
                    color = Muted,
                    style = MaterialTheme.typography.bodyMedium,
                )
            }
        }
        // A room first, the title afterwards: people (and my other devices) join meanwhile.
        item(key = "new") {
            Column(Modifier.widthIn(max = 640.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                ActionButton(
                    if (opening) tr("Opening a room…") else tr("New room"),
                    {
                        if (opening) return@ActionButton
                        opening = true
                        problem = null
                        scope.launch {
                            try {
                                openEmptyRoom(core, nav)
                            } catch (e: CoreException) {
                                problem = friendly(e)
                            } finally {
                                opening = false
                            }
                        }
                    },
                    if (invites.isEmpty() && mine.isEmpty()) Modifier.initialFocus() else Modifier,
                    primary = invites.isEmpty() && mine.isEmpty(),
                    icon = Icons.Add,
                )
                Text(tr("Invite people first, choose what to watch afterwards."), color = Muted, style = MaterialTheme.typography.bodySmall)
                problem?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodyMedium) }
            }
        }
        if (loaded && invites.isEmpty() && mine.isEmpty()) {
            item(key = "empty") {
                Text(
                    if (TogetherInbox.unreachable && TogetherInbox.rooms == null) {
                        tr("Can't reach your AlwyWatch server right now. Trying again…")
                    } else {
                        tr("No rooms yet. Start one here, or open a movie or an episode and choose Watch together.")
                    },
                    Modifier.widthIn(max = 640.dp).padding(top = 12.dp),
                    style = MaterialTheme.typography.bodyLarge,
                )
            }
        }
        if (invites.isNotEmpty()) {
            item(key = "h-invites") { ListHeader(tr("Invitations")) }
            items(invites, key = { "i" + it.code }) { room ->
                InvitationRow(room, Modifier.widthIn(max = 760.dp), first = room == invites.first())
            }
        }
        if (mine.isNotEmpty()) {
            item(key = "h-mine") { ListHeader(tr("Your rooms")) }
            items(mine, key = { "m" + it.code }) { room ->
                MyRoomRow(room, Modifier.widthIn(max = 760.dp), first = invites.isEmpty() && room == mine.first())
            }
        }
    }
}

@Composable
private fun ListHeader(text: String) {
    Text(text.uppercase(), Modifier.padding(top = 8.dp, start = 4.dp), style = MaterialTheme.typography.labelSmall, color = Subtle)
}

/** "<host> invites you": Join / Not now. */
@Composable
private fun InvitationRow(
    room: TogetherRoom,
    modifier: Modifier = Modifier,
    first: Boolean,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    Row(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(d.radius + 4.dp))
            .background(SurfaceColor)
            .padding(14.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
        verticalAlignment = Alignment.Top,
    ) {
        RoomPoster(room.media, if (d.isTv) 84.dp else 64.dp)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Avatar(room.hostAvatar, room.host, room.hostUserKey, 28.dp)
                Text(tr("{name} invites you", "name" to room.host), style = MaterialTheme.typography.bodyMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Text(room.media.label(), style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text(roomStatus(room), style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 2)
            Row(Modifier.padding(top = 4.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                ActionButton(
                    if (busy) tr("Joining…") else tr("Join"),
                    {
                        if (!busy) {
                            busy = true
                            error = null
                            scope.launch {
                                try {
                                    joinRoom(core, nav, room.code)
                                } catch (e: CoreException) {
                                    error = friendly(e)
                                } finally {
                                    busy = false
                                }
                            }
                        }
                    },
                    Modifier.rememberFocus("together", "join:${room.code}", default = first),
                    primary = true,
                    icon = Icons.Play,
                )
                ActionButton(tr("Not now"), {
                    val code = room.code
                    TogetherInbox.forget(code)
                    ctx.services.scope.launch { runCatching { core.togetherDecline(code) } }
                }, Modifier.rememberFocus("together", "decline:${room.code}"))
            }
            error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
        }
    }
}

private fun roomStatus(room: TogetherRoom): String {
    val others = room.members.size
    val who =
        when {
            others == 0 -> null
            others == 1 -> room.members.first()
            else -> trn("{name} and {n} other", "{name} and {n} others", others - 1, "name" to room.members.first())
        }
    val now = room.positionMs.toLong() + if (room.playing) (room.nowMs - room.atMs).coerceAtLeast(0) else 0
    return listOfNotNull(
        if (room.started) tr("Watching now · {time}", "time" to formatTime(now)) else tr("Waiting room"),
        who,
    ).joinToString(" · ")
}

/** A room I'm in: opens the Room screen (or the player once it started). */
@Composable
private fun MyRoomRow(
    room: TogetherRoom,
    modifier: Modifier = Modifier,
    first: Boolean,
) {
    val d = LocalDimens.current
    val nav = LocalNavigator.current
    val open = { nav.push(Route.Room(room.code)) }
    FocusCard(
        onClick = open,
        modifier = modifier.fillMaxWidth().rememberFocus("together", "room:${room.code}", default = first),
        glow = false,
        focusScale = 1.02f,
    ) {
        Row(
            Modifier.fillMaxWidth().background(SurfaceColor).padding(10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            RoomPoster(room.media, if (d.isTv) 84.dp else 64.dp)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(room.media.label(), style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(
                    if (room.hosting) tr("You're the host") else tr("Hosted by {name}", "name" to room.host),
                    style = MaterialTheme.typography.bodyMedium,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(roomStatus(room), style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 2)
            }
            if (!d.isTv) ActionButton(tr("Open"), open, primary = true)
        }
    }
}

/** Home: invitations waiting for an answer, as a row (nothing when there are none). */
@Composable
fun TogetherRail() {
    val d = LocalDimens.current
    val invites =
        TogetherInbox.rooms
            ?.rooms
            .orEmpty()
            .filter { it.invited }
    if (invites.isEmpty()) return
    Column(Modifier.padding(top = if (d.isTv) 18.dp else 24.dp)) {
        SectionHeader(tr("Invitations to watch together"), subtitle = tr("Join or answer later"))
        LazyRow(
            contentPadding = PaddingValues(horizontal = d.gutter, vertical = if (d.isTv) 14.dp else 12.dp),
            horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
        ) {
            items(invites, key = { it.code }) { room ->
                FocusCard(
                    onClick = { TogetherInbox.prompt = room },
                    modifier = Modifier.width(if (d.isTv) 460.dp else 320.dp),
                    glow = false,
                    focusScale = 1.03f,
                ) {
                    Row(
                        Modifier.fillMaxWidth().background(SurfaceColor).padding(10.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        RoomPoster(room.media, if (d.isTv) 72.dp else 56.dp)
                        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Avatar(room.hostAvatar, room.host, room.hostUserKey, 24.dp)
                                Text(
                                    tr("{name} invites you", "name" to room.host),
                                    style = MaterialTheme.typography.bodyMedium,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                )
                            }
                            Text(room.media.label(), style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            Text(if (room.started) tr("Watching now") else tr("Waiting room"), style = MaterialTheme.typography.bodySmall, color = Muted)
                        }
                    }
                }
            }
        }
    }
}

// ----- Invitation prompt ------------------------------------------------------------------------

/**
 * A new invitation, wherever the user is in the app: Join, or Not now (declines). BACK closes
 * it without answering: the invitation stays on the Together page.
 */
@Composable
fun InvitationPrompt() {
    val room = TogetherInbox.prompt ?: return
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var busy by remember(room.code) { mutableStateOf(false) }
    var error by remember(room.code) { mutableStateOf<String?>(null) }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = { TogetherInbox.prompt = null },
        containerColor = Background,
        icon = { Avatar(room.hostAvatar, room.host, room.hostUserKey, 56.dp) },
        title = { Text(tr("{name} invites you to watch together", "name" to room.host)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(room.media.label(), style = MaterialTheme.typography.titleMedium)
                Text(
                    if (room.started) tr("They're already watching: you'll join where they are.") else tr("They're in the waiting room."),
                    color = Muted,
                    style = MaterialTheme.typography.bodyMedium,
                )
                error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            ActionButton(
                if (busy) tr("Joining…") else tr("Join"),
                {
                    if (!busy) {
                        busy = true
                        error = null
                        scope.launch {
                            try {
                                joinRoom(core, nav, room.code)
                                TogetherInbox.prompt = null
                            } catch (e: CoreException) {
                                error = friendly(e)
                            } finally {
                                busy = false
                            }
                        }
                    }
                },
                Modifier.focusOnAppear(),
                primary = true,
                icon = Icons.Play,
            )
        },
        dismissButton = {
            ActionButton(tr("Not now"), {
                val code = room.code
                TogetherInbox.forget(code)
                ctx.services.scope.launch { runCatching { core.togetherDecline(code) } }
            })
        },
    )
}

// ----- Room screen ------------------------------------------------------------------------------

/**
 * A watch together room: the title, who is in it, who is invited. The host invites people and
 * starts; everyone else waits. When it starts, the player opens in sync for everyone here.
 */
@Composable
fun RoomScreen(code: String) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var problem by remember(code) { mutableStateOf<String?>(null) }
    var joining by remember(code) { mutableStateOf(TogetherSession.seat?.code != code) }
    LaunchedEffect(code) {
        if (TogetherSession.seat?.code != code) {
            try {
                val j = core.togetherJoin(code)
                if (TogetherSession.seat != null) leaveCurrentRoom()
                TogetherSession.begin(core, j)
            } catch (e: CoreException) {
                problem = friendly(e)
            }
            joining = false
        }
    }
    // The room's heartbeat while this screen is visible (the player takes over when it opens).
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        var attached = false
        val observer =
            androidx.lifecycle.LifecycleEventObserver { _, e ->
                when (e) {
                    androidx.lifecycle.Lifecycle.Event.ON_START ->
                        if (!attached) {
                            attached = true
                            TogetherSession.attach(core)
                        }
                    androidx.lifecycle.Lifecycle.Event.ON_STOP ->
                        if (attached) {
                            attached = false
                            TogetherSession.detach()
                        }
                    else -> {}
                }
            }
        lifecycle.addObserver(observer)
        onDispose {
            lifecycle.removeObserver(observer)
            if (attached) TogetherSession.detach()
        }
    }
    val seat = TogetherSession.seat?.takeIf { it.code == code }
    val state = TogetherSession.state?.takeIf { seat != null && it.code == code }
    // The room went away (or we left it from the player): say why, or just go back.
    var hadSeat by remember(code) { mutableStateOf(false) }
    LaunchedEffect(seat != null, joining) {
        if (seat != null) {
            hadSeat = true
        } else if (hadSeat && !joining) {
            val why = TogetherSession.ended
            TogetherSession.ended = null
            if (why != null) problem = why else nav.pop()
        }
    }
    // Each start opens the player by itself, wherever the app is (MainActivity).
    val started = state?.started == true
    // Back in the waiting room after watching something together.
    val again = (state?.round ?: 0uL) > 0uL
    var inviting by remember { mutableStateOf(false) }
    var confirmLeave by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    val inviteButton = remember { FocusRequester() }
    // Leaving clears the seat: the screen then goes back by itself.
    val leave = { leaveCurrentRoom() }
    if (confirmLeave) {
        val st = TogetherSession.state?.takeIf { it.code == code }
        val mySeat = TogetherSession.seat?.takeIf { it.code == code }
        val hosting =
            st != null &&
                mySeat != null &&
                be.alwyapps.watch.core
                    .togetherIsHost(st, mySeat.memberId)
        val myKey = st?.members?.firstOrNull { it.id == mySeat?.memberId }?.userKey
        val othersHere = st?.members.orEmpty().any { it.userKey != myKey }
        ConfirmDialog(
            tr("Leave this room?"),
            when {
                hosting && othersHere -> tr("You're the host: the room stays open for the others and one of them becomes the host.")
                hosting -> tr("You're alone in it: the room closes, and the invitations you sent are withdrawn.")
                else -> tr("To come back, you'll need a new invitation.")
            },
            confirm = tr("Leave"),
            onConfirm = leave,
            onDismiss = { confirmLeave = false },
        )
    }
    if (inviting && state != null && seat != null) {
        InvitePeopleDialog(state, seat.memberId) {
            inviting = false
            if (d.isTv) scope.launch { runCatching { inviteButton.requestFocus() } }
        }
    }
    // TV: no scrolling by a third of the screen when the buttons take the focus (that pushed
    // the title off the top).
    MinimalScrollSpec {
        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(horizontal = d.gutter, vertical = 24.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp),
        ) {
            item(key = "title") {
                PageTitle(
                    tr("Watch together"),
                    subtitle =
                        if (state == null) {
                            null
                        } else if (started) {
                            tr("Watching now")
                        } else {
                            tr("Waiting room")
                        },
                )
            }
            if (state == null || seat == null) {
                item(key = "wait") {
                    Column(Modifier.widthIn(max = 640.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                        Text(problem ?: tr("Opening the room…"), style = MaterialTheme.typography.bodyLarge)
                        if (problem != null) ActionButton(tr("Back"), { nav.pop() }, Modifier.initialFocus(), primary = true, icon = Icons.Back)
                    }
                }
                return@LazyColumn
            }
            val host = state.members.firstOrNull { it.id == state.host }
            val hostName = host?.name ?: tr("the host")
            val isHost =
                be.alwyapps.watch.core
                    .togetherIsHost(state, seat.memberId)
            val myKey = state.members.firstOrNull { it.id == seat.memberId }?.userKey
            val untitled = state.media.untitled()
            item(key = "media") {
                Row(Modifier.widthIn(max = 760.dp), horizontalArrangement = Arrangement.spacedBy(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    RoomPoster(state.media, if (d.isTv) 110.dp else 88.dp)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(state.media.label(), style = MaterialTheme.typography.headlineSmall, maxLines = 3, overflow = TextOverflow.Ellipsis)
                        Text(
                            if (isHost) tr("You're the host") else tr("Hosted by {name}", "name" to hostName),
                            style = MaterialTheme.typography.bodyMedium,
                            color = Muted,
                        )
                        if (!started) {
                            Text(
                                when {
                                    untitled && isHost ->
                                        tr("Open a movie or an episode and choose Watch in your room. The people you invite can join in the meantime.")
                                    untitled -> tr("{name} is choosing what to watch", "name" to hostName)
                                    isHost && again ->
                                        tr(
                                            "Start this again, or pick another title: open a movie or an episode and choose Watch in your room. Everyone stays here.",
                                        )
                                    isHost -> tr("Invite people, then start when everyone is here. Play, pause and seeking follow everyone.")
                                    again -> tr("{name} is picking what to watch next", "name" to hostName)
                                    else -> tr("Waiting for {name} to start", "name" to hostName)
                                },
                                style = MaterialTheme.typography.bodyMedium,
                                color = if (isHost) Muted else OnSurface,
                            )
                        }
                    }
                }
            }
            item(key = "actions") {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    androidx.compose.foundation.layout.FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        fun start(from: Long) {
                            if (busy) return
                            busy = true
                            problem = null
                            scope.launch {
                                try {
                                    TogetherSession.accept(core.togetherStart(code, seat.memberId, from.coerceAtLeast(0).toULong()))
                                } catch (e: CoreException) {
                                    problem = friendly(e)
                                } finally {
                                    busy = false
                                }
                            }
                        }
                        // Host, nobody else in the room or invited yet: Invite first (OK by reflex
                        // must not start an empty room); Start once someone is there.
                        val alone = state.members.all { it.userKey == myKey } && state.invited.isEmpty()
                        when {
                            // Takes the focus when the room starts (it was on Leave in the waiting room).
                            started ->
                                ActionButton(tr("Watch with them"), {
                                    scope.launch { openRoomPlayer(ctx, core)?.let { problem = it } }
                                }, Modifier.focusOnAppear(), primary = true, icon = Icons.Play)
                            isHost && untitled ->
                                ActionButton(
                                    tr("Choose what to watch"),
                                    { nav.reset(Route.Home) },
                                    if (alone) Modifier else Modifier.initialFocus(),
                                    primary = !alone,
                                    // Not Play: it opens the libraries to pick a title.
                                    icon = Icons.Library,
                                )
                            isHost -> {
                                val resume = TogetherSession.startAt
                                ActionButton(
                                    if (busy) {
                                        tr("Starting…")
                                    } else if (resume > 0) {
                                        tr("Start at {time}", "time" to formatTime(resume))
                                    } else {
                                        tr("Start watching")
                                    },
                                    { start(resume) },
                                    if (alone) Modifier else Modifier.initialFocus(),
                                    primary = !alone,
                                    icon = Icons.Play,
                                )
                                if (resume > 0) ActionButton(tr("From the beginning"), { start(0) })
                            }
                        }
                        if (isHost) {
                            ActionButton(
                                tr("Invite people"),
                                { inviting = true },
                                Modifier.focusRequester(inviteButton).then(if (alone && !started) Modifier.initialFocus() else Modifier),
                                primary = alone && !started,
                                icon = Icons.Add,
                            )
                        }
                        ActionButton(
                            tr("Leave"),
                            { confirmLeave = true },
                            if (!isHost && !started) Modifier.initialFocus() else Modifier,
                            icon = Icons.Close,
                        )
                    }
                    (problem ?: TogetherSession.problem.takeIf { started })?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodyMedium) }
                }
            }
            item(key = "members") {
                val people = state.members.people()
                Group(if (people.size == 1) tr("In the room") else tr("In the room · {count}", "count" to people.size), Modifier.widthIn(max = 760.dp)) {
                    people.forEachIndexed { i, (m, devices) ->
                        if (i > 0) Divider()
                        ListRow(
                            if (m.userKey == myKey) tr("{name} (you)", "name" to m.name) else m.name,
                            subtitle =
                                listOfNotNull(
                                    tr("Host").takeIf { host != null && m.userKey == host.userKey },
                                    trn("on {n} device", "on {n} devices", devices).takeIf { devices > 1 },
                                ).joinToString(" · ").ifEmpty { null },
                            leading = { Avatar(m.avatar, m.name, m.userKey, if (d.isTv) 44.dp else 40.dp) },
                        )
                    }
                }
            }
            if (state.invited.isNotEmpty()) {
                item(key = "invited") {
                    Group(tr("Invited"), Modifier.widthIn(max = 760.dp), footer = tr("They can join until they say no.")) {
                        state.invited.forEachIndexed { i, p ->
                            if (i > 0) Divider()
                            ListRow(
                                p.name,
                                subtitle = tr("Hasn't joined yet"),
                                leading = { Avatar(p.avatar, p.name, p.userKey, if (d.isTv) 44.dp else 40.dp) },
                                trailing =
                                    if (isHost) {
                                        {
                                            IconAction(Icons.Close, tr("Remove {name}", "name" to p.name), {
                                                scope.launch {
                                                    try {
                                                        TogetherSession.accept(core.togetherUninvite(code, seat.memberId, p.userKey))
                                                    } catch (e: CoreException) {
                                                        problem = friendly(e)
                                                    }
                                                }
                                            })
                                        }
                                    } else {
                                        null
                                    },
                            )
                        }
                    }
                }
            }
        }
    }
}

/**
 * The host picks people to invite: the ones they last watched with first, then everyone by
 * name. Several at once, then one "Send" (TV: OK ticks a row).
 */
@Composable
internal fun InvitePeopleDialog(
    state: TogetherState,
    memberId: String,
    onClose: () -> Unit,
) {
    val core = LocalCore.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var people by remember { mutableStateOf<List<TogetherPerson>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var selected by remember { mutableStateOf(listOf<String>()) }
    var query by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        people =
            try {
                core.togetherPeople()
            } catch (e: CoreException) {
                error = friendly(e)
                emptyList()
            }
    }
    val present = (state.members.map { it.userKey } + state.invited.map { it.userKey }).toSet()
    // Recent partners first (most recent first), then everyone by name: the "Recent" and
    // "Everyone" headers below rely on that order (the server's order put no recent one first,
    // so the headers never showed).
    val all =
        people
            .orEmpty()
            .filter { it.userKey !in present }
            .sortedWith(compareBy<TogetherPerson>({ it.lastTogetherAt == null }, { -(it.lastTogetherAt ?: 0L) }, { it.name.lowercase() }))
    val q = query.trim()
    val shown = if (q.isEmpty()) all else all.filter { it.name.contains(q, true) || it.mediaName.contains(q, true) }
    val sameName =
        all
            .groupBy { it.name.lowercase() }
            .filter { it.value.size > 1 }
            .keys
    val firstRow = remember { FocusRequester() }
    LaunchedEffect(people != null) {
        if (people != null && d.isTv) {
            androidx.compose.runtime.withFrameNanos { }
            runCatching { firstRow.requestFocus() }
        }
    }

    fun send() {
        if (busy || selected.isEmpty()) return
        busy = true
        error = null
        scope.launch {
            try {
                for (chunk in selected.chunked(20)) {
                    TogetherSession.accept(core.togetherInvite(state.code, memberId, chunk))
                }
                onClose()
            } catch (e: CoreException) {
                error = friendly(e)
            } finally {
                busy = false
            }
        }
    }
    val sendFocus = remember { FocusRequester() }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onClose,
        containerColor = Background,
        title = { Text(tr("Invite people")) },
        text = {
            // A dialog of its own: the screen's "focus once" is already used.
            CompositionLocalProvider(LocalInitialFocusDone provides null) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (all.size > 8) {
                        Field(
                            query,
                            { query = it },
                            Modifier.fillMaxWidth().dpadEscape(),
                            placeholder = { Text(tr("Search by name")) },
                            singleLine = true,
                            leadingIcon = Icons.Search,
                        )
                    }
                    when {
                        people == null -> Text(tr("Loading…"), color = Muted)
                        all.isEmpty() ->
                            Text(
                                error ?: if (people.orEmpty().isEmpty()) {
                                    tr("Nobody else uses this AlwyWatch server yet.")
                                } else {
                                    tr("Everyone is already here or invited.")
                                },
                                color = Muted,
                            )
                        shown.isEmpty() -> Text(tr("Nobody by that name."), color = Muted)
                    }
                    androidx.compose.foundation.lazy.LazyColumn(Modifier.heightIn(max = if (d.isTv) 380.dp else 440.dp)) {
                        shown.forEachIndexed { i, p ->
                            val recent = p.lastTogetherAt != null
                            if (q.isEmpty() && recent && i == 0) item(key = "h-recent") { DialogHeader(tr("Recent")) }
                            if (q.isEmpty() && !recent && (i == 0 || shown[i - 1].lastTogetherAt != null) && shown.first().lastTogetherAt != null) {
                                item(key = "h-all") { DialogHeader(tr("Everyone")) }
                            }
                            item(key = p.userKey) {
                                val on = p.userKey in selected
                                ListRow(
                                    p.name,
                                    (if (i == 0) Modifier.focusRequester(firstRow) else Modifier)
                                        // Down from the last person: Send, not Cancel (the nearest one).
                                        .then(if (i == shown.lastIndex) Modifier.focusProperties { down = sendFocus } else Modifier),
                                    subtitle =
                                        listOfNotNull(
                                            p.mediaName.takeIf { it.isNotEmpty() && !it.equals(p.name, true) },
                                            p.serverName.takeIf { it.isNotEmpty() && p.name.lowercase() in sameName },
                                        ).joinToString(" · ").ifEmpty { null },
                                    leading = { Avatar(p.avatar, p.name, p.userKey, 40.dp) },
                                    onClick = { selected = if (on) selected - p.userKey else selected + p.userKey },
                                    trailing = { Tick(on) },
                                )
                            }
                        }
                    }
                    error?.takeIf { all.isNotEmpty() }?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
                }
            }
        },
        confirmButton = {
            val n = selected.size
            ActionButton(
                when {
                    busy -> tr("Sending…")
                    n == 0 -> tr("Send invitations")
                    else -> trn("Send {n} invitation", "Send {n} invitations", n)
                },
                { send() },
                Modifier.focusRequester(sendFocus),
                primary = n > 0,
            )
        },
        dismissButton = { ActionButton(tr("Cancel"), onClose) },
    )
}

@Composable
private fun DialogHeader(text: String) {
    Text(text.uppercase(), Modifier.padding(start = 16.dp, top = 10.dp, bottom = 4.dp), style = MaterialTheme.typography.labelSmall, color = Subtle)
}

/** A round check mark: ticked or not. */
@Composable
private fun Tick(on: Boolean) {
    Box(
        Modifier
            .size(26.dp)
            .clip(CircleShape)
            .background(if (on) Accent else androidx.compose.ui.graphics.Color.Transparent)
            .then(if (on) Modifier else Modifier.background(SurfaceColor, CircleShape)),
        contentAlignment = Alignment.Center,
    ) {
        if (on) {
            Icon(Icons.Check, tr("Selected"), Modifier.size(18.dp), tint = androidx.compose.ui.graphics.Color.Black)
        } else {
            Box(
                Modifier
                    .size(22.dp)
                    .clip(CircleShape)
                    .background(Outline),
            )
        }
    }
}

/**
 * "Watch together" from the player: the room for what's playing, over the paused video. The
 * host invites people and starts from where they are, without leaving the player.
 */
@Composable
fun PlayerRoomDialog(
    positionMs: Long,
    onStart: () -> Unit,
    onCancel: () -> Unit,
) {
    val d = LocalDimens.current
    val state = TogetherSession.state
    val seat = TogetherSession.seat
    var inviting by remember { mutableStateOf(false) }
    if (inviting && state != null && seat != null) {
        InvitePeopleDialog(state, seat.memberId) { inviting = false }
        return
    }
    val myKey = state?.members?.firstOrNull { it.id == seat?.memberId }?.userKey
    val alone = state == null || (state.members.all { it.userKey == myKey } && state.invited.isEmpty())
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onCancel,
        containerColor = Background,
        title = { Text(tr("Watch together")) },
        text = {
            CompositionLocalProvider(LocalInitialFocusDone provides null) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(
                        state?.media?.label() ?: tr("Opening a room…"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        tr("Paused for now. Invite people, then start: everyone starts at {time} at the same moment.", "time" to formatTime(positionMs)),
                        color = Muted,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    if (state != null) {
                        androidx.compose.foundation.lazy.LazyColumn(Modifier.heightIn(max = if (d.isTv) 300.dp else 360.dp)) {
                            state.members.people().forEach { (m, _) ->
                                item(key = "m" + m.id) {
                                    ListRow(
                                        if (m.userKey == myKey) tr("{name} (you)", "name" to m.name) else m.name,
                                        subtitle = tr("In the room"),
                                        leading = { Avatar(m.avatar, m.name, m.userKey, 40.dp) },
                                    )
                                }
                            }
                            state.invited.forEach { p ->
                                item(key = "i" + p.userKey) {
                                    ListRow(
                                        p.name,
                                        subtitle = tr("Invited"),
                                        leading = { Avatar(p.avatar, p.name, p.userKey, 40.dp) },
                                    )
                                }
                            }
                        }
                    }
                    androidx.compose.foundation.layout.FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(10.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        ActionButton(
                            tr("Invite people"),
                            { inviting = true },
                            if (alone) Modifier.initialFocus() else Modifier,
                            primary = alone,
                            icon = Icons.Add,
                        )
                        ActionButton(
                            tr("Start at {time}", "time" to formatTime(positionMs)),
                            onStart,
                            if (alone) Modifier else Modifier.initialFocus(),
                            primary = !alone,
                            icon = Icons.Play,
                        )
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { ActionButton(tr("Cancel"), onCancel, icon = Icons.Close) },
    )
}
