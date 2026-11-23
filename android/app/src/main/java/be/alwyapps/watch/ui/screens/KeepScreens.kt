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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.relocation.bringIntoViewRequester
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
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
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
import be.alwyapps.watch.ui.ambientGlow
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.rememberSwr
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import kotlinx.coroutines.launch
import kotlin.math.abs

private const val DAY = 86_400L

/** True when the date has passed but the owner has not turned deletion on: nothing happens yet. */
fun LeavingItem.paused(now: Long = System.currentTimeMillis() / 1000): Boolean =
    !collectionActive && protection == Protection.NONE && (effectiveDeletionDate ?: Long.MAX_VALUE) < now

/** "Leaves in 4 days", "Not scheduled for deletion", "Kept forever"… */
fun LeavingItem.leavesLabel(): String {
    val now = System.currentTimeMillis() / 1000
    return when (protection) {
        Protection.FOREVER -> tr("Kept forever")
        Protection.UNTIL_WATCHED -> tr("Kept until watched")
        else -> {
            val d = effectiveDeletionDate ?: return tr("No date")
            if (paused(now)) return tr("Not scheduled for deletion")
            val days = (d - now) / DAY
            val base =
                when {
                    days < 0 -> tr("Any day now")
                    days == 0L -> tr("Leaves today")
                    days == 1L -> tr("Leaves tomorrow")
                    else -> trn("Leaves in {n} day", "Leaves in {n} days", days)
                }
            if (protection == Protection.EXTENDED) tr("{status} (extended)", "status" to base) else base
        }
    }
}

/** Under the keep buttons: how one person's choice weighs. Same sentence in every app. */
fun keepHint(): String = tr("Everyone's choices count and the longest one wins. “Fine to delete”: it only goes if nobody else keeps it.")

/** Short countdown for poster badges: "3 days", "Today", "Overdue", "No date", "Kept". */
fun LeavingItem.shortLabel(): Pair<String, Boolean> {
    val now = System.currentTimeMillis() / 1000
    if (protection == Protection.FOREVER || protection == Protection.UNTIL_WATCHED) return tr("Kept") to false
    // Past its date but deletion isn't turned on for its collection: nothing is planned yet.
    if (paused(now)) return tr("No date") to false
    val d = effectiveDeletionDate ?: return tr("Soon") to false
    val days = (d - now) / DAY
    return when {
        days < 0 -> tr("Any day now") to true
        days == 0L -> tr("Today") to true
        days == 1L -> trn("{n} day", "{n} days", 1) to true
        else -> trn("{n} day", "{n} days", days) to (days <= 7)
    }
}

fun LeavingItem.voteLabel(): String? =
    myVote?.let {
        when (it.kind) {
            VoteKind.DONT_CARE -> tr("You're fine with it going")
            VoteKind.KEEP_DAYS -> tr("You keep it a while")
            VoteKind.KEEP_UNTIL_WATCHED -> tr("You keep it until you watch it")
            VoteKind.KEEP_FOREVER -> it.reason?.let { r -> tr("You keep it forever ({reason})", "reason" to r) } ?: tr("You keep it forever")
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
    /** Given the first choice, to move the focus there when the choices appear. */
    firstFocus: androidx.compose.ui.focus.FocusRequester? = null,
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
            item {
                ActionButton(
                    tr("Until I watch it"),
                    { send(req(VoteKind.KEEP_UNTIL_WATCHED, reminder = 7u)) },
                    (if (focusFirst) Modifier.initialFocus() else Modifier)
                        .then(firstFocus?.let { Modifier.focusRequester(it) } ?: Modifier),
                    primary = true,
                    icon = be.alwyapps.watch.ui.Icons.Schedule,
                )
            }
            item { ActionButton(tr("Keep 30 days"), { send(req(VoteKind.KEEP_DAYS, days = 30u)) }) }
            item { ActionButton(tr("Keep 7 days"), { send(req(VoteKind.KEEP_DAYS, days = 7u)) }) }
            item { ActionButton(tr("Keep forever"), { send(req(VoteKind.KEEP_FOREVER)) }, icon = be.alwyapps.watch.ui.Icons.FavoriteFilled) }
            item { ActionButton(tr("Fine to delete"), { send(req(VoteKind.DONT_CARE)) }) }
            if (item.myVote != null) item { ActionButton(tr("Remove my choice"), { send(null) }, icon = be.alwyapps.watch.ui.Icons.Close) }
        }
        Text(keepHint(), color = Muted, style = MaterialTheme.typography.bodySmall)
        error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
    }
}

/** Compact rail for the home screen. */
@Composable
fun LeavingSoonRail(onFocus: ((LeavingItem) -> Unit)? = null) {
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
    // Up from the cards goes to the header's button: at the far right, a geometric search
    // from a card on the left found the row above first and the button was out of reach.
    val sort = remember { FocusRequester() }
    // TV: the home pins the focused element just under the hero. For this header button that
    // left a thin strip of the row above showing; instead the whole row (header and cards) is
    // placed where a focused card would put it.
    val cards =
        remember {
            androidx.compose.foundation.relocation
                .BringIntoViewRequester()
        }
    val scope = rememberCoroutineScope()
    val rowPad = with(androidx.compose.ui.platform.LocalDensity.current) { 12.dp.toPx() }
    Column(Modifier.padding(top = if (d.isTv) 18.dp else 24.dp)) {
        Row(Modifier.padding(horizontal = d.gutter), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Column(Modifier.weight(1f)) {
                Text(tr("Leaving soon"), style = MaterialTheme.typography.titleMedium)
                Text(tr("Keep what you still want to watch"), style = MaterialTheme.typography.bodySmall, color = Muted)
            }
            ActionButton(
                tr("Go through them one by one"),
                { nav.push(Route.Triage) },
                Modifier
                    .focusRequester(sort)
                    .then(
                        if (d.isTv) {
                            Modifier.onFocusChanged { f ->
                                if (f.isFocused) {
                                    scope.launch {
                                        // After the button's own scroll request (this one wins).
                                        androidx.compose.runtime.withFrameNanos { }
                                        // The cards' top (inside the row's padding): where a focused card goes.
                                        runCatching {
                                            cards.bringIntoView(
                                                androidx.compose.ui.geometry
                                                    .Rect(0f, rowPad, 1f, rowPad + 1f),
                                            )
                                        }
                                    }
                                }
                            }
                        } else {
                            Modifier
                        },
                    ),
                primary = true,
                icon = be.alwyapps.watch.ui.Icons.Favorite,
            )
            if (!d.isTv) {
                be.alwyapps.watch.ui
                    .IconAction(be.alwyapps.watch.ui.Icons.ChevronRight, tr("See all"), { nav.push(Route.LeavingSoon) })
            }
        }
        // Like the other rows on TV: the focused card at a fixed spot, clear of the side menu.
        be.alwyapps.watch.ui.TvScrollSpec(leading = d.gutter) {
            LazyRow(
                Modifier.bringIntoViewRequester(cards),
                contentPadding = PaddingValues(horizontal = d.gutter, vertical = 12.dp),
                horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
            ) {
                items(items, key = { it.key }) { item ->
                    Column(Modifier.width(d.posterWidth)) {
                        FocusCard(
                            onClick = { nav.push(Route.LeavingSoon) },
                            Modifier.focusProperties { up = sort },
                            // The home's hero follows it (title, year, when it leaves).
                            onFocus = onFocus?.let { f -> { f(item) } },
                        ) {
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
                item(key = "see-all") {
                    be.alwyapps.watch.ui
                        .SeeAllTile(d.posterWidth, d.posterWidth * 1.5f) { nav.push(Route.LeavingSoon) }
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
    var filter by remember { mutableStateOf("all") }
    val all = list.value.orEmpty()
    val now = System.currentTimeMillis() / 1000
    val shown =
        when (filter) {
            "urgent" -> all.filter { it.shortLabel().second }
            "mine" -> all.filter { it.myVote != null }
            "kept" -> all.filter { it.protection != Protection.NONE }
            else -> all
        }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        item {
            Column(verticalArrangement = Arrangement.spacedBy(14.dp), modifier = Modifier.padding(bottom = 6.dp)) {
                be.alwyapps.watch.ui.PageTitle(
                    tr("Leaving soon"),
                    listOf(
                        trn("{count} title", "{count} titles", all.size, "count" to "%,d".format(all.size)),
                        tr("{count} with your choice", "count" to all.count { it.myVote != null }),
                    ).joinToString(" · "),
                )
                Text(
                    tr("What the server will delete, soonest first. Keep what you still want to watch or love."),
                    color = Muted,
                    style = MaterialTheme.typography.bodyMedium,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ActionButton(
                        tr("Go through them one by one"),
                        { nav.push(Route.Triage) },
                        Modifier.initialFocus(),
                        primary = true,
                        icon = be.alwyapps.watch.ui.Icons.Favorite,
                    )
                    if (core.keepConnection()?.isAdmin == true) {
                        ActionButton(tr("Admin"), { nav.push(Route.KeepAdmin) }, icon = be.alwyapps.watch.ui.Icons.Settings)
                    }
                }
                LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for ((key, label) in listOf("all" to tr("All"), "urgent" to tr("Within a week"), "mine" to tr("My choices"), "kept" to tr("Kept"))) {
                        item(key) { ActionButton(label, { filter = key }, selected = filter == key) }
                    }
                }
            }
        }
        val paused = all.count { it.paused(now) }
        if (paused > 0) {
            item("paused") {
                Text(
                    tr(
                        "{count} of these are not scheduled: their date has passed but the server's owner hasn't turned deletion on yet. Your choices still count when it is.",
                        "count" to "%,d".format(paused),
                    ),
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(d.radius))
                        .background(SurfaceColor)
                        .padding(14.dp),
                    style = MaterialTheme.typography.bodySmall,
                    color = Muted,
                )
            }
        }
        list.error?.let { msg ->
            // Unreachable server: say which, and that the list shown is the last one it sent.
            val text =
                if (list.failure is CoreException.Network) {
                    val at = runCatching { core.keepConnection()?.baseUrl }.getOrNull()
                    when {
                        at != null && all.isNotEmpty() ->
                            tr("Can't reach your AlwyWatch server ({address}). This is the last list it sent.", "address" to at)
                        at != null -> tr("Can't reach your AlwyWatch server ({address}).", "address" to at)
                        all.isNotEmpty() -> tr("Can't reach your AlwyWatch server. This is the last list it sent.")
                        else -> tr("Can't reach your AlwyWatch server.")
                    }
                } else {
                    msg
                }
            item {
                be.alwyapps.watch.ui
                    .ErrorBanner(text) { list.reload() }
            }
        }
        items(shown, key = { it.key }) { item ->
            val expanded = open == item.key
            Column(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(d.radius + 4.dp))
                    .background(if (expanded) be.alwyapps.watch.ui.SurfaceHigh else SurfaceColor),
            ) {
                // No zoom: inside the row's rounded clip a zoomed card lost its outline at the
                // edges, the only focus cue left.
                FocusCard(
                    onClick = { open = if (expanded) null else item.key },
                    glow = false,
                    modifier = Modifier.fillMaxWidth(),
                    shape = RoundedCornerShape(d.radius + 4.dp),
                    focusScale = 1f,
                ) {
                    // TV: a small poster (3 to 4 rows on screen instead of 1.5).
                    Row(Modifier.padding(if (d.isTv) 8.dp else 10.dp), verticalAlignment = Alignment.CenterVertically) {
                        UrlImage(
                            item.posterUrl,
                            80.dp,
                            Modifier.width(if (d.isTv) 44.dp else 58.dp).aspectRatio(2f / 3f).clip(RoundedCornerShape(if (d.isTv) 6.dp else 10.dp)),
                        )
                        Column(Modifier.padding(start = 14.dp).weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(
                                listOfNotNull(item.title, item.year?.toString()).joinToString(" · "),
                                style = MaterialTheme.typography.titleSmall,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            val (label, urgent) = item.shortLabel()
                            be.alwyapps.watch.ui
                                .Badge(label, color = if (urgent) Danger else be.alwyapps.watch.ui.SurfaceHigh)
                            val c = item.counts
                            val others =
                                (c.forever + c.untilWatched + c.days).toInt() - if (item.myVote != null && item.myVote!!.kind != VoteKind.DONT_CARE) 1 else 0
                            listOfNotNull(
                                item.voteLabel(),
                                if (others > 0) trn("{n} other keeps it", "{n} others keep it", others) else null,
                            ).joinToString(" · ").ifEmpty { null }?.let {
                                Text(it, color = if (item.myVote != null) Accent else Muted, style = MaterialTheme.typography.bodySmall, maxLines = 1)
                            }
                        }
                        androidx.compose.material3.Icon(
                            if (expanded) be.alwyapps.watch.ui.Icons.Close else be.alwyapps.watch.ui.Icons.ChevronRight,
                            null,
                            Modifier.padding(horizontal = 8.dp),
                            tint = be.alwyapps.watch.ui.Subtle,
                        )
                    }
                }
                if (expanded) {
                    KeepActions(item, onVoted = { list.reload() }, Modifier.padding(start = 12.dp, end = 12.dp, bottom = 14.dp), focusFirst = true)
                }
            }
        }
        if (list.value != null && shown.isEmpty()) {
            item { Text(tr("Nothing here."), Modifier.padding(vertical = 24.dp), color = Muted) }
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
    val total = (queue?.size ?: 0) + done
    Column(
        Modifier
            .fillMaxSize()
            .triageGlow()
            .verticalScroll(rememberScrollState())
            .padding(d.gutter),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text(tr("One by one"), style = MaterialTheme.typography.headlineMedium)
        if (total > 0) {
            Box(
                Modifier
                    .width(220.dp)
                    .height(4.dp)
                    .clip(RoundedCornerShape(50))
                    .background(be.alwyapps.watch.ui.SurfaceHigh),
            ) {
                Box(
                    Modifier
                        .fillMaxWidth(done.toFloat() / total)
                        .height(4.dp)
                        .background(Accent),
                )
            }
        }
        error?.let { Text(it, color = Danger) }
        when {
            queue == null -> Text(tr("Loading…"), color = Muted)
            item == null -> Text(if (done > 0) tr("All done, thanks! {count} sorted.", "count" to done) else tr("Nothing to sort right now."), color = Muted)
            else -> {
                Text(
                    if (d.isTv) {
                        trn(
                            "{n} left  ·  ← fine to delete   → keep until I watch it   ↑ keep forever",
                            "{n} left  ·  ← fine to delete   → keep until I watch it   ↑ keep forever",
                            queue!!.size,
                        )
                    } else {
                        trn(
                            "{n} left  ·  Swipe left: fine to delete · right: keep until I watch it · up: keep forever",
                            "{n} left  ·  Swipe left: fine to delete · right: keep until I watch it · up: keep forever",
                            queue!!.size,
                        )
                    },
                    color = be.alwyapps.watch.ui.Subtle,
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
                                    Key.DirectionUp -> vote(req(VoteKind.KEEP_FOREVER))
                                    else -> return@onKeyEvent false
                                }
                                true
                            }.pointerInput(item.key) {
                                // Distance or a quick flick decides (people swipe fast).
                                val tracker =
                                    androidx.compose.ui.input.pointer.util
                                        .VelocityTracker()

                                fun decide() {
                                    val v = tracker.calculateVelocity()
                                    val flick = 1200f
                                    when {
                                        (dy < -250 || v.y < -flick) && abs(dy) > abs(dx) -> vote(req(VoteKind.KEEP_FOREVER))
                                        dx > 250 || v.x > flick -> vote(req(VoteKind.KEEP_UNTIL_WATCHED, reminder = 7u))
                                        dx < -250 || v.x < -flick -> vote(req(VoteKind.DONT_CARE))
                                    }
                                    dx = 0f
                                    dy = 0f
                                    tracker.resetTracking()
                                }
                                detectDragGestures(
                                    onDragEnd = { decide() },
                                    onDragCancel = { decide() },
                                    onDrag = { change, amount ->
                                        change.consume()
                                        tracker.addPosition(change.uptimeMillis, change.position)
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
                    ActionButton(
                        tr("Until I watch it"),
                        { vote(req(VoteKind.KEEP_UNTIL_WATCHED, reminder = 7u)) },
                        primary = true,
                        icon = be.alwyapps.watch.ui.Icons.Schedule,
                    )
                    ActionButton(tr("Forever"), { vote(req(VoteKind.KEEP_FOREVER)) }, icon = be.alwyapps.watch.ui.Icons.FavoriteFilled)
                    ActionButton(tr("Keep 30 days"), { vote(req(VoteKind.KEEP_DAYS, days = 30u)) })
                    ActionButton(tr("Fine to delete"), { vote(req(VoteKind.DONT_CARE)) }, icon = be.alwyapps.watch.ui.Icons.Close)
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
                    androidx.compose.foundation.layout.FlowRow(
                        horizontalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterHorizontally),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) { buttons() }
                }
            }
        }
    }
}

/** Admins: per-user rights and the collection countdown. */
@Composable
fun KeepAdminScreen() {
    val core = LocalCore.current
    val nav = be.alwyapps.watch.ui.LocalNavigator.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var users by remember { mutableStateOf<List<AdminUser>?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var armed by remember { mutableStateOf<Pair<Long, UInt>?>(null) }
    val items = remember { core.cachedLeavingSoon().orEmpty() }
    val collections = items.mapNotNull { i -> i.collectionId?.let { it to (i.collectionTitle ?: tr("Collection {id}", "id" to it)) } }.distinct()
    // Seerr's users, to link people to the Seerr account they already have.
    var seerrUsers by remember { mutableStateOf<List<be.alwyapps.watch.core.keep.SeerrUser>>(emptyList()) }
    var linking by remember { mutableStateOf<AdminUser?>(null) }
    LaunchedEffect(Unit) {
        try {
            users = core.keepAdminUsers()
        } catch (e: CoreException) {
            message = e.message
        }
        seerrUsers = runCatching { core.keepAdminSeerrUsers() }.getOrDefault(emptyList())
    }
    linking?.let { u ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { linking = null },
            containerColor = be.alwyapps.watch.ui.Background,
            icon = {
                be.alwyapps.watch.ui
                    .Avatar(u.avatar, u.name, u.userKey, 48.dp)
            },
            title = { Text(tr("Requests of {name}", "name" to u.name)) },
            text = {
                LazyColumn(Modifier.heightIn(max = 420.dp)) {
                    item {
                        be.alwyapps.watch.ui.ListRow(
                            tr("Automatic"),
                            subtitle = tr("Their own Seerr account (created if needed)"),
                            onClick = {
                                linking = null
                                scope.launch {
                                    runCatching { core.keepAdminLinkSeerr(u.userKey, null) }.onSuccess { nu ->
                                        users = users?.map { if (it.userKey == nu.userKey) nu else it }
                                    }
                                }
                            },
                            trailing = {
                                if (u.seerrUserId == null) {
                                    be.alwyapps.watch.ui
                                        .Value("✓")
                                }
                            },
                        )
                    }
                    items(seerrUsers.size) { i ->
                        val su = seerrUsers[i]
                        be.alwyapps.watch.ui.ListRow(
                            su.name,
                            onClick = {
                                linking = null
                                scope.launch {
                                    try {
                                        val nu = core.keepAdminLinkSeerr(u.userKey, su.id)
                                        users = users?.map { if (it.userKey == nu.userKey) nu else it }
                                    } catch (e: CoreException) {
                                        message = e.message
                                    }
                                }
                            },
                            trailing = {
                                if (u.seerrUserId == su.id) {
                                    be.alwyapps.watch.ui
                                        .Value("✓")
                                }
                            },
                        )
                    }
                }
            },
            confirmButton = { ActionButton(tr("Close"), { linking = null }) },
        )
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
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(22.dp),
    ) {
        item {
            be.alwyapps.watch.ui
                .PageTitle(tr("Admin"), tr("Leaving soon: countdown and people's rights"))
        }
        item {
            be.alwyapps.watch.ui.Group(tr("Kept forever")) {
                be.alwyapps.watch.ui.ListRow(
                    tr("Who keeps what forever"),
                    subtitle = tr("Per person, and how much of it someone else keeps too"),
                    icon = be.alwyapps.watch.ui.Icons.FavoriteFilled,
                    onClick = { nav.push(Route.KeepForever) },
                    trailing = {
                        be.alwyapps.watch.ui
                            .Chevron()
                    },
                    modifier = Modifier.initialFocus(),
                )
            }
        }
        message?.let { item { Text(it, color = Accent) } }
        item {
            be.alwyapps.watch.ui.Group(
                tr("Start countdown"),
                footer =
                    tr(
                        "Every item is already past its date. Before activating a collection in Maintainerr, restart its countdown so people get the full delay to vote. This changes deletion dates in Maintainerr (press twice to confirm).",
                    ),
            ) {
                collections.forEachIndexed { i, (id, title) ->
                    if (i > 0) {
                        be.alwyapps.watch.ui
                            .Divider()
                    }
                    be.alwyapps.watch.ui.ListRow(title, icon = be.alwyapps.watch.ui.Icons.Schedule, trailing = {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            for (stagger in listOf(0u, 14u, 30u)) {
                                val label = if (stagger == 0u) tr("Restart now") else trn("Spread over {n} day", "Spread over {n} days", stagger.toLong())
                                val isArmed = armed == id to stagger
                                ActionButton(if (isArmed) tr("Confirm") else label, {
                                    if (!isArmed) {
                                        armed = id to stagger
                                    } else {
                                        armed = null
                                        scope.launch {
                                            message =
                                                try {
                                                    val r = core.keepAdminCountdown(id, stagger)
                                                    tr(
                                                        "{collection}: {reset} restarted, {skipped} skipped",
                                                        "collection" to title,
                                                        "reset" to r.reset,
                                                        "skipped" to r.skipped,
                                                    )
                                                } catch (e: CoreException) {
                                                    e.message
                                                }
                                        }
                                    }
                                }, primary = isArmed)
                            }
                        }
                    })
                }
            }
        }
        item {
            be.alwyapps.watch.ui.Group(
                tr("People"),
                footer =
                    if (LocalDimens.current.isTv) {
                        tr("Everyone can keep forever by default. Select a value to change it.")
                    } else {
                        tr("Everyone can keep forever by default. Tap a value to change it.")
                    },
            ) {
                users.orEmpty().forEachIndexed { i, u ->
                    if (i > 0) {
                        be.alwyapps.watch.ui
                            .Divider()
                    }
                    val r = u.rights
                    Column(Modifier.padding(bottom = 12.dp)) {
                        val sameName = users.orEmpty().count { it.name.equals(u.name, true) } > 1
                        be.alwyapps.watch.ui.ListRow(
                            if (u.isAdmin) tr("{name} · admin", "name" to u.name) else u.name,
                            subtitle =
                                listOfNotNull(
                                    u.mediaName.takeIf { it.isNotEmpty() && (sameName || !it.equals(u.name, true)) },
                                    if (u.userKey.startsWith("plex:")) "Plex" else "Jellyfin",
                                    tr("{count} kept forever", "count" to u.foreverCount),
                                ).joinToString(" · "),
                            leading = {
                                be.alwyapps.watch.ui
                                    .Avatar(u.avatar, u.name, u.userKey, if (d.isTv) 40.dp else 34.dp)
                            },
                        )
                        LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            item {
                                ActionButton(if (r.canKeepForever) tr("Forever allowed") else tr("Forever blocked"), {
                                    update(u, r.copy(canKeepForever = !r.canKeepForever))
                                }, primary = !r.canKeepForever)
                            }
                            item {
                                ActionButton(tr("Max forever: {value}", "value" to (r.maxForever ?: "∞")), {
                                    val next = listOf<UInt?>(null, 5u, 10u, 25u)
                                    update(u, r.copy(maxForever = next[(next.indexOf(r.maxForever) + 1) % next.size]))
                                })
                            }
                            if (seerrUsers.isNotEmpty()) {
                                item {
                                    val linked = seerrUsers.firstOrNull { it.id == u.seerrUserId }?.name
                                    ActionButton(tr("Requests as: {name}", "name" to (linked ?: tr("automatic"))), { linking = u })
                                }
                            }
                            item {
                                ActionButton(tr("Max days: {value}", "value" to (r.maxKeepDays ?: "∞")), {
                                    val next = listOf<UInt?>(null, 7u, 30u, 90u)
                                    update(u, r.copy(maxKeepDays = next[(next.indexOf(r.maxKeepDays) + 1) % next.size]))
                                })
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun TriageInfo(item: LeavingItem) {
    Text(item.title, style = MaterialTheme.typography.headlineMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
    val (label, urgent) = item.shortLabel()
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        item.year?.let { Text(it.toString(), color = Muted) }
        be.alwyapps.watch.ui
            .Badge(label, color = if (urgent) Danger else be.alwyapps.watch.ui.SurfaceHigh)
    }
    val c = item.counts
    val keepers = c.forever + c.untilWatched + c.days
    if (keepers > 0u) Text(trn("{n} person keeps it", "{n} people keep it", keepers.toLong()), color = Muted, style = MaterialTheme.typography.bodySmall)
}

/** Warm glow behind the triage card. */
private fun Modifier.triageGlow() = this.then(Modifier.ambientGlow(Accent.copy(alpha = 0.5f)))
