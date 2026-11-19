// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectDragGestures
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.AdminUser
import be.alwyapps.watch.core.keep.LeavingItem
import be.alwyapps.watch.core.keep.Protection
import be.alwyapps.watch.core.keep.Rights
import be.alwyapps.watch.core.keep.VoteKind
import be.alwyapps.watch.core.keep.VoteRequest
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.UrlImage
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.rememberSwr
import kotlinx.coroutines.launch
import kotlin.math.abs

private const val DAY = 86_400L

/** "Leaves in 4 days", "Overdue (collection not active yet)", "Kept forever"… */
fun LeavingItem.leavesLabel(): String {
    val now = System.currentTimeMillis() / 1000
    return when (protection) {
        Protection.FOREVER -> "Kept forever"
        Protection.UNTIL_WATCHED -> "Kept until watched"
        else -> {
            val d = effectiveDeletionDate ?: return "No date"
            val days = (d - now) / DAY
            val base =
                when {
                    days < 0 -> "Overdue"
                    days == 0L -> "Leaves today"
                    days == 1L -> "Leaves tomorrow"
                    else -> "Leaves in $days days"
                }
            val extra = if (protection == Protection.EXTENDED) " (extended)" else ""
            base + extra + if (!collectionActive) " · not active yet" else ""
        }
    }
}

/** Short countdown for poster badges: "3 days", "Today", "Overdue", "Kept". */
fun LeavingItem.shortLabel(): Pair<String, Boolean> {
    val now = System.currentTimeMillis() / 1000
    if (protection == Protection.FOREVER || protection == Protection.UNTIL_WATCHED) return "Kept" to false
    val d = effectiveDeletionDate ?: return "Soon" to false
    val days = (d - now) / DAY
    return when {
        days < 0 -> "Overdue" to true
        days == 0L -> "Today" to true
        days == 1L -> "1 day" to true
        else -> "$days days" to (days <= 7)
    }
}

fun LeavingItem.voteLabel(): String? =
    myVote?.let {
        when (it.kind) {
            VoteKind.DONT_CARE -> "You don't care"
            VoteKind.KEEP_DAYS -> "You keep it a while"
            VoteKind.KEEP_UNTIL_WATCHED -> "You keep it until you watch it"
            VoteKind.KEEP_FOREVER -> "You keep it forever" + (it.reason?.let { r -> " ($r)" } ?: "")
        }
    }

private fun req(
    kind: VoteKind,
    days: UInt? = null,
    reminder: UInt? = null,
    reason: String? = null,
) = VoteRequest(kind, days, reminder, reason)

/** The keep choices for one item. */
@Composable
fun KeepActions(
    item: LeavingItem,
    onVoted: (LeavingItem) -> Unit,
    modifier: Modifier = Modifier,
    focusFirst: Boolean = false,
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var error by remember(item.key) { mutableStateOf<String?>(null) }

    fun send(r: VoteRequest?) {
        scope.launch {
            try {
                onVoted(if (r == null) core.keepUnvote(item.key) else core.keepVote(item.key, r))
                error = null
            } catch (e: CoreException) {
                error = e.message
            }
        }
    }
    Column(modifier, verticalArrangement = Arrangement.spacedBy(8.dp)) {
        LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            item { ActionButton("Keep 7 days", { send(req(VoteKind.KEEP_DAYS, days = 7u)) }, if (focusFirst) Modifier.initialFocus() else Modifier) }
            item { ActionButton("Keep 30 days", { send(req(VoteKind.KEEP_DAYS, days = 30u)) }) }
            item { ActionButton("Until I watch it", { send(req(VoteKind.KEEP_UNTIL_WATCHED, reminder = 7u)) }, primary = true) }
            item { ActionButton("Forever: I love it", { send(req(VoteKind.KEEP_FOREVER, reason = "love it")) }) }
            item { ActionButton("Forever: a classic", { send(req(VoteKind.KEEP_FOREVER, reason = "classic")) }) }
            item { ActionButton("I don't care", { send(req(VoteKind.DONT_CARE)) }) }
            if (item.myVote != null) item { ActionButton("Remove my choice", { send(null) }) }
        }
        error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
    }
}

/** Compact rail for the home screen. */
@Composable
fun LeavingSoonRail() {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val list = rememberSwr("leaving-rail", cached = { core.cachedLeavingSoon() }, refresh = { core.refreshLeavingSoon(it) })
    val items =
        list.value
            ?.filter { it.protection == Protection.NONE || it.protection == Protection.EXTENDED }
            ?.take(15)
            .orEmpty()
    if (items.isEmpty()) return
    Column(Modifier.padding(top = if (d.isTv) 18.dp else 24.dp)) {
        Row(Modifier.padding(horizontal = d.gutter), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Column(Modifier.weight(1f)) {
                Text("Leaving soon", style = MaterialTheme.typography.titleMedium)
                Text("Keep what you still want to watch", style = MaterialTheme.typography.bodySmall, color = Muted)
            }
            ActionButton("Sort them out", { nav.push(Route.Triage) }, primary = true, icon = be.alwyapps.watch.ui.Icons.Favorite)
            if (!d.isTv) {
                be.alwyapps.watch.ui
                    .IconAction(be.alwyapps.watch.ui.Icons.ChevronRight, "See all", { nav.push(Route.LeavingSoon) })
            }
        }
        LazyRow(contentPadding = PaddingValues(horizontal = d.gutter, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(d.railSpacing)) {
            items(items, key = { it.key }) { item ->
                Column(Modifier.width(d.posterWidth)) {
                    FocusCard(onClick = { nav.push(Route.LeavingSoon) }) {
                        UrlImage(item.posterUrl, d.posterWidth, Modifier.fillMaxWidth().aspectRatio(2f / 3f))
                        val (label, urgent) = item.shortLabel()
                        be.alwyapps.watch.ui.Badge(
                            label,
                            Modifier.align(Alignment.TopStart).padding(8.dp),
                            color = if (urgent) be.alwyapps.watch.ui.Danger else Color.Black.copy(alpha = 0.6f),
                        )
                    }
                    if (!d.isTv) {
                        Text(
                            item.title,
                            Modifier.padding(top = 8.dp),
                            style = MaterialTheme.typography.titleSmall,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            listOfNotNull(
                                item.year?.toString(),
                                item.kind.replaceFirstChar {
                                    it.uppercase()
                                },
                            ).joinToString(" · "),
                            style = MaterialTheme.typography.bodySmall,
                            color = Muted,
                            maxLines = 1,
                        )
                    }
                }
            }
        }
    }
}

/** Everything leaving, most urgent first, with the keep choices. */
@Composable
fun LeavingSoonScreen() {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val list = rememberSwr("leaving", cached = { core.cachedLeavingSoon() }, refresh = { core.refreshLeavingSoon(it) })
    var open by remember { mutableStateOf<String?>(null) }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(d.gutter), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Leaving soon", Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                ActionButton("Sort them out", { nav.push(Route.Triage) }, Modifier.initialFocus(), primary = true)
                if (core.keepConnection()?.isAdmin == true) ActionButton("Admin", { nav.push(Route.KeepAdmin) })
            }
            Text("What the server will delete, soonest first. Keep what you want to watch or love.", color = Muted)
        }
        list.error?.let { item { Text(it, color = Danger) } }
        items(list.value.orEmpty(), key = { it.key }) { item ->
            Column(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(8.dp))
                    .background(SurfaceColor)
                    .padding(10.dp),
            ) {
                FocusCard(onClick = { open = if (open == item.key) null else item.key }, modifier = Modifier.fillMaxWidth()) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        UrlImage(item.posterUrl, 70.dp, Modifier.width(if (d.isTv) 80.dp else 60.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(6.dp)))
                        Column(Modifier.padding(start = 12.dp).weight(1f)) {
                            Text(
                                listOfNotNull(item.title, item.year?.toString()).joinToString(" · "),
                                style = MaterialTheme.typography.titleMedium,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            Text(item.leavesLabel(), color = if (item.protection == Protection.NONE) Accent else Muted)
                            item.voteLabel()?.let { Text(it, color = Muted, style = MaterialTheme.typography.bodySmall) }
                            val c = item.counts
                            val others = c.forever + c.untilWatched + c.days
                            if (others > 0u) Text("$others keep request(s)", color = Muted, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
                if (open == item.key) {
                    KeepActions(item, onVoted = { list.reload() }, Modifier.padding(top = 10.dp), focusFirst = true)
                }
            }
        }
    }
}

/**
 * Swipe mode: one card at a time, most urgent first, only items I have not voted on.
 * Phone: left = don't care, right = keep until I watch it, up = keep forever.
 * TV/keyboard: the same with the D-pad; the buttons are always there too.
 */
@Composable
fun TriageScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var queue by remember { mutableStateOf<List<LeavingItem>?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var done by remember { mutableIntStateOf(0) }
    LaunchedEffect(Unit) {
        try {
            queue = core.keepTriage(50u)
        } catch (e: CoreException) {
            error = e.message
        }
    }

    fun vote(r: VoteRequest) {
        val item = queue?.firstOrNull() ?: return
        queue = queue?.drop(1)
        done++
        scope.launch {
            try {
                core.keepVote(item.key, r)
            } catch (e: CoreException) {
                error = e.message
            }
        }
    }
    val item = queue?.firstOrNull()
    val focus = remember { FocusRequester() }
    var dx by remember { mutableFloatStateOf(0f) }
    var dy by remember { mutableFloatStateOf(0f) }
    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(d.gutter),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("Sort them out", style = MaterialTheme.typography.titleLarge)
        error?.let { Text(it, color = Danger) }
        when {
            queue == null -> Text("Loading…", color = Muted)
            item == null -> Text(if (done > 0) "All done, thanks! $done sorted." else "Nothing to sort right now.", color = Muted)
            else -> {
                Text(
                    "${queue!!.size} left · ← don't care   → keep until I watch it   ↑ keep forever",
                    color = Muted,
                    style = MaterialTheme.typography.bodySmall,
                )
                LaunchedEffect(item.key) { runCatching { focus.requestFocus() } }
                val card: @Composable () -> Unit = {
                    Box(
                        Modifier
                            .width(if (d.isTv) 220.dp else 260.dp)
                            .focusRequester(focus)
                            .focusable()
                            .onKeyEvent {
                                if (it.type != KeyEventType.KeyDown) return@onKeyEvent false
                                when (it.key) {
                                    Key.DirectionLeft -> vote(req(VoteKind.DONT_CARE))
                                    Key.DirectionRight -> vote(req(VoteKind.KEEP_UNTIL_WATCHED, reminder = 7u))
                                    Key.DirectionUp -> vote(req(VoteKind.KEEP_FOREVER, reason = "love it"))
                                    else -> return@onKeyEvent false
                                }
                                true
                            }.pointerInput(item.key) {
                                detectDragGestures(
                                    onDragEnd = {
                                        when {
                                            dy < -250 && abs(dy) > abs(dx) -> vote(req(VoteKind.KEEP_FOREVER, reason = "love it"))
                                            dx > 250 -> vote(req(VoteKind.KEEP_UNTIL_WATCHED, reminder = 7u))
                                            dx < -250 -> vote(req(VoteKind.DONT_CARE))
                                        }
                                        dx = 0f
                                        dy = 0f
                                    },
                                    onDrag = { change, amount ->
                                        change.consume()
                                        dx += amount.x
                                        dy += amount.y
                                    },
                                )
                            }.graphicsLayer {
                                translationX = dx
                                translationY = dy.coerceAtMost(0f)
                                rotationZ = dx / 40f
                            },
                    ) {
                        Column(Modifier.clip(RoundedCornerShape(12.dp)).background(SurfaceColor)) {
                            UrlImage(item.posterUrl, 300.dp, Modifier.fillMaxWidth().aspectRatio(2f / 3f))
                            // TV: title and buttons sit beside the poster (a 1080p TV is 540 dp tall).
                            if (!d.isTv) Column(Modifier.padding(12.dp)) { TriageInfo(item) }
                        }
                    }
                }
                val buttons: @Composable () -> Unit = {
                    ActionButton("Don't care", { vote(req(VoteKind.DONT_CARE)) })
                    ActionButton("Keep 30 days", { vote(req(VoteKind.KEEP_DAYS, days = 30u)) })
                    ActionButton("Until I watch it", { vote(req(VoteKind.KEEP_UNTIL_WATCHED, reminder = 7u)) }, primary = true)
                    ActionButton("Forever", { vote(req(VoteKind.KEEP_FOREVER, reason = "love it")) })
                }
                if (d.isTv) {
                    Row(horizontalArrangement = Arrangement.spacedBy(32.dp), verticalAlignment = Alignment.CenterVertically) {
                        card()
                        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            TriageInfo(item)
                            buttons()
                        }
                    }
                } else {
                    card()
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) { buttons() }
                }
            }
        }
    }
}

/** Admins: per-user rights and the collection countdown. */
@Composable
fun KeepAdminScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var users by remember { mutableStateOf<List<AdminUser>?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var armed by remember { mutableStateOf<Pair<Long, UInt>?>(null) }
    val items = remember { core.cachedLeavingSoon().orEmpty() }
    val collections = items.mapNotNull { i -> i.collectionId?.let { it to (i.collectionTitle ?: "Collection $it") } }.distinct()
    LaunchedEffect(Unit) {
        try {
            users = core.keepAdminUsers()
        } catch (e: CoreException) {
            message = e.message
        }
    }

    fun update(
        u: AdminUser,
        r: Rights,
    ) {
        scope.launch {
            try {
                val nu = core.keepAdminSetRights(u.userKey, r)
                users = users?.map { if (it.userKey == nu.userKey) nu else it }
            } catch (e: CoreException) {
                message = e.message
            }
        }
    }
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(d.gutter), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        item { Text("Admin", style = MaterialTheme.typography.titleLarge) }
        message?.let { item { Text(it, color = Accent) } }
        item {
            Text("Start countdown", style = MaterialTheme.typography.titleMedium)
            Text(
                "Every item is already past its date. Before activating a collection in Maintainerr, restart its countdown so people get the full delay to vote. This changes deletion dates in Maintainerr.",
                color = Muted,
            )
        }
        items(collections, key = { "c${it.first}" }) { (id, title) ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(title, Modifier.weight(1f))
                for (stagger in listOf(0u, 14u, 30u)) {
                    val label = if (stagger == 0u) "Today" else "Spread $stagger d"
                    val isArmed = armed == id to stagger
                    ActionButton(if (isArmed) "Confirm: $label" else label, {
                        if (!isArmed) {
                            armed = id to stagger
                        } else {
                            armed = null
                            scope.launch {
                                message =
                                    try {
                                        val r = core.keepAdminCountdown(id, stagger)
                                        "$title: ${r.reset} restarted, ${r.skipped} skipped"
                                    } catch (e: CoreException) {
                                        e.message
                                    }
                            }
                        }
                    }, primary = isArmed)
                }
            }
        }
        item { Text("People", Modifier.padding(top = 12.dp), style = MaterialTheme.typography.titleMedium) }
        items(users.orEmpty(), key = { it.userKey }) { u ->
            Column(
                Modifier
                    .fillMaxWidth()
                    .heightIn(min = 48.dp)
                    .clip(RoundedCornerShape(8.dp))
                    .background(SurfaceColor)
                    .padding(10.dp),
            ) {
                Text(u.name + if (u.isAdmin) " (admin)" else "")
                Text("${u.foreverCount} kept forever", color = Muted, style = MaterialTheme.typography.bodySmall)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 6.dp)) {
                    val r = u.rights
                    ActionButton(if (r.canKeepForever) "Forever: allowed" else "Forever: blocked", { update(u, r.copy(canKeepForever = !r.canKeepForever)) })
                    ActionButton("Max forever: ${r.maxForever ?: "∞"}", {
                        val next = listOf<UInt?>(null, 5u, 10u, 25u)
                        update(u, r.copy(maxForever = next[(next.indexOf(r.maxForever) + 1) % next.size]))
                    })
                    ActionButton("Max days: ${r.maxKeepDays ?: "∞"}", {
                        val next = listOf<UInt?>(null, 7u, 30u, 90u)
                        update(u, r.copy(maxKeepDays = next[(next.indexOf(r.maxKeepDays) + 1) % next.size]))
                    })
                }
            }
        }
    }
}

@Composable
private fun TriageInfo(item: LeavingItem) {
    Text(listOfNotNull(item.title, item.year?.toString()).joinToString(" · "), style = MaterialTheme.typography.titleMedium)
    Text(item.leavesLabel(), color = Accent)
}
