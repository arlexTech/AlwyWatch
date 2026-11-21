// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithCache
import androidx.compose.ui.focus.FocusDirection
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.media.ImageKind
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind
import kotlinx.coroutines.launch

// ----- Item helpers ---------------------------------------------------------------------------

/** 2:3 artwork; for an episode, its show's poster (its own primary image is a still). */
fun Item.poster() =
    images.firstOrNull { it.kind == ImageKind.SERIES_PRIMARY }.takeIf { kind == ItemKind.EPISODE }
        ?: images.firstOrNull { it.kind == ImageKind.PRIMARY }

fun Item.backdrop() = images.firstOrNull { it.kind == ImageKind.BACKDROP }

fun Item.logo() = images.firstOrNull { it.kind == ImageKind.LOGO }

/** 16:9 artwork: an episode's own still, otherwise the backdrop. */
fun Item.thumb() =
    if (kind == ItemKind.EPISODE) {
        images.firstOrNull { it.kind == ImageKind.PRIMARY } ?: images.firstOrNull { it.kind == ImageKind.THUMB }
    } else {
        images.firstOrNull { it.kind == ImageKind.THUMB } ?: backdrop()
    } ?: poster()

fun Item.subtitle(): String =
    when (kind) {
        ItemKind.EPISODE ->
            listOfNotNull(
                if (seasonNumber != null && episodeNumber != null) "S$seasonNumber · E$episodeNumber" else null,
                title.takeIf { seriesTitle != null },
            ).joinToString("  ")
        ItemKind.SERIES -> listOfNotNull(year?.toString(), childCount?.let { "$it season${if (it > 1u) "s" else ""}" }).joinToString(" · ")
        else -> listOfNotNull(year?.toString(), runtimeMs?.takeIf { it > 0u }?.let { formatRuntime(it.toLong()) }).joinToString(" · ")
    }

fun formatRuntime(ms: Long): String {
    val min = ms / 60_000
    return if (min >= 60) "${min / 60} h ${min % 60} min" else "$min min"
}

/** 0..1 when partly watched, else null. */
fun Item.progress(): Float? {
    val runtime = runtimeMs?.toLong() ?: return null
    val pos = user.positionMs.toLong()
    return if (pos > 0 && runtime > 0) (pos.toFloat() / runtime).coerceIn(0f, 1f) else null
}

fun Item.remaining(): String? {
    val runtime = runtimeMs?.toLong() ?: return null
    val left = runtime - user.positionMs.toLong()
    return if (user.positionMs > 0u && left > 0) "${formatRuntime(left)} left" else null
}

/** Ambient color of an item's artwork (BlurHash average, no decoding needed). */
fun Item.ambient(): Color? = BlurHash.averageColor((backdrop() ?: poster())?.blurhash)

// ----- Focus ----------------------------------------------------------------------------------

/** The dark line inside the TV focus outline (visible on white posters). */
private val FocusEdge = Color.Black.copy(alpha = 0.7f)

/**
 * Clickable surface with a clear focus state: on TV it scales up, gets a white outline and a
 * soft glow; on touch screens it is a plain clickable. `clickable` handles D-pad center/Enter.
 */
@Composable
fun FocusCard(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    shape: Shape = RoundedCornerShape(LocalDimens.current.radius),
    onFocus: (() -> Unit)? = null,
    onLongClick: (() -> Unit)? = null,
    // The TV focus glow is a shadow: behind see-through content it shows as a grey band.
    glow: Boolean = true,
    // Wide rows grow less (the same ratio on a full-width row reaches under the side menu).
    focusScale: Float? = null,
    content: @Composable BoxScope.() -> Unit,
) {
    val d = LocalDimens.current
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    if (focused && onFocus != null) LaunchedEffect(Unit) { onFocus() }
    val scale by animateFloatAsState(if (focused) focusScale ?: d.focusScale else 1f, tween(160), label = "scale")
    Box(
        modifier
            .pageFocusTracked(focused)
            // Long press (touch, or holding OK on a remote): the item's menu. Before the layer:
            // the focused element is then measured without its zoom. Inside it, scrolling the
            // focused card into place followed the zoom animation, and every ←/→ lifted the
            // page a little.
            .combinedClickable(interactionSource = interaction, indication = null, onLongClick = onLongClick, onClick = onClick)
            // One layer for the scale and the rounded clip (a clip is a layer of its own
            // otherwise: two per card in every row).
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
                this.shape = shape
                clip = true
            }
            // TV focus: scale and a white border. (A 24 dp white shadow, redrawn on every frame of
            // the scale animation, was too much for Chromecast-class GPUs.) A thin dark line just
            // inside the white keeps the outline visible on white artwork; two flat strokes, no blur.
            .then(if (focused && d.isTv) Modifier.border(2.5.dp, Color.White, shape).border(3.5.dp, FocusEdge, shape) else Modifier),
        content = content,
    )
}

/** The long-press action for items that have a menu (movies, shows, episodes). */
private fun Item.menuable(open: () -> Unit): (() -> Unit)? = if (kind == ItemKind.MOVIE || kind == ItemKind.SERIES || kind == ItemKind.EPISODE) open else null

/**
 * Bumped when an item's state changes from a menu (watched…): screens showing lists reload.
 */
object ItemsChanged {
    var version by androidx.compose.runtime.mutableIntStateOf(0)
        private set

    fun bump() {
        version++
    }
}

/**
 * For what a long press on OK opens: the OK still held (its repeats and its release) would
 * click whatever takes the focus. Ignored until OK is pressed again.
 */
@Composable
fun Modifier.ignoreHeldOk(): Modifier {
    var armed by remember { mutableStateOf(false) }
    return onPreviewKeyEvent { e ->
        val ok =
            e.nativeKeyEvent.keyCode == android.view.KeyEvent.KEYCODE_DPAD_CENTER ||
                e.nativeKeyEvent.keyCode == android.view.KeyEvent.KEYCODE_ENTER ||
                e.nativeKeyEvent.keyCode == android.view.KeyEvent.KEYCODE_NUMPAD_ENTER
        when {
            !ok || armed -> false
            e.type == KeyEventType.KeyDown && e.nativeKeyEvent.repeatCount == 0 -> {
                armed = true
                false
            }
            else -> true
        }
    }
}

/** Long-press menu of a movie, show or episode: watched state, show/season, download. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
fun ItemMenu(
    item: Item,
    inContinueWatching: Boolean = false,
    onDismiss: () -> Unit,
) {
    val core = LocalCore.current
    val account = LocalItemOwners.current[item.id] ?: LocalAccount.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val ctx = androidx.compose.ui.platform.LocalContext.current
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val played = item.user.played
    androidx.compose.material3.ModalBottomSheet(
        onDismissRequest = onDismiss,
        // Fully open: half open, the rows below the fold took the focus out of sight (TV).
        sheetState = androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true),
        // On TV a full-width sheet is a wall of empty space: a compact panel instead.
        sheetMaxWidth = if (d.isTv) 460.dp else androidx.compose.material3.BottomSheetDefaults.SheetMaxWidth,
        containerColor = Background,
        // On TV the handle would take the focus first (it is only for touch).
        dragHandle =
            if (d.isTv) {
                null
            } else {
                (
                    {
                        androidx.compose.material3.BottomSheetDefaults
                            .DragHandle()
                    }
                )
            },
    ) {
        Column(
            Modifier
                .ignoreHeldOk()
                .verticalScroll(androidx.compose.foundation.rememberScrollState())
                .padding(bottom = 24.dp)
                .navigationBarsPadding(),
        ) {
            Text(
                if (item.kind == ItemKind.EPISODE) "${item.seriesTitle ?: ""} · ${item.subtitle()}" else item.title,
                Modifier.padding(horizontal = 20.dp, vertical = 8.dp),
                style = MaterialTheme.typography.titleMedium,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )

            fun act(block: suspend () -> Unit) {
                onDismiss()
                scope.launch {
                    runCatching { block() }
                    ItemsChanged.bump()
                }
            }
            val started = item.user.positionMs > 0u
            if (started && (item.kind == ItemKind.EPISODE || item.kind == ItemKind.MOVIE)) {
                ListRow(
                    "Play from start",
                    subtitle = "Instead of resuming at ${be.alwyapps.watch.player.formatTime(item.user.positionMs.toLong())}",
                    icon = Icons.Play,
                    onClick = {
                        onDismiss()
                        ctx.startActivity(
                            be.alwyapps.watch.player.PlayerActivity
                                .intent(ctx, account, item.id, 0L),
                        )
                    },
                )
            }
            if (!played) {
                ListRow("Mark watched", icon = Icons.Check, onClick = { act { core.setPlayed(account, item.id, true) } })
            }
            // Watched, or partly watched: back to unwatched (forgets where it stopped).
            if (played || started) {
                ListRow("Mark unwatched", subtitle = if (started && !played) "Forgets where you stopped" else null, icon = Icons.Refresh, onClick = {
                    act { core.setPlayed(account, item.id, false) }
                })
            }
            // Only for what is in that row: started, or shown there (the next episode of a show).
            if (!played && (started || inContinueWatching) && (item.kind == ItemKind.EPISODE || item.kind == ItemKind.MOVIE)) {
                ListRow("Remove from Continue watching", icon = Icons.Close, onClick = {
                    act { core.removeFromContinueWatching(account, item.id) }
                })
            }
            val sid = item.seriesId
            if (item.kind == ItemKind.EPISODE && sid != null) {
                ListRow("Go to the show", icon = Icons.Tv, onClick = {
                    onDismiss()
                    nav.push(Route.Detail(sid, account = account))
                })
                item.seasonId?.let { season ->
                    ListRow("Go to season ${item.seasonNumber ?: ""}".trim(), icon = Icons.Library, onClick = {
                        onDismiss()
                        nav.push(Route.Detail(sid, season, account))
                    })
                }
            }
            ListRow("Details", icon = Icons.Info, onClick = {
                onDismiss()
                nav.push(Route.Detail(item.id, account = account))
            })
            if (!d.isTv && (item.kind == ItemKind.MOVIE || item.kind == ItemKind.EPISODE)) {
                ListRow("Download", icon = Icons.Download, onClick = {
                    act {
                        core.downloadItem(account, item.id, null)
                        be.alwyapps.watch.downloads.DownloadWorker
                            .start(ctx)
                    }
                })
            }
        }
    }
}

// ----- Cards ----------------------------------------------------------------------------------

/** Drawn in one pass (two rounded clips before: two layers on every Continue watching card). */
@Composable
private fun ProgressBar(
    fraction: Float,
    modifier: Modifier = Modifier,
) {
    Spacer(
        modifier
            .fillMaxWidth()
            .height(4.dp)
            .drawBehind {
                val r =
                    androidx.compose.ui.geometry
                        .CornerRadius(size.height / 2)
                drawRoundRect(Color.White.copy(alpha = 0.25f), cornerRadius = r)
                drawRoundRect(Accent, size = size.copy(width = size.width * fraction), cornerRadius = r)
            },
    )
}

@Composable
fun Badge(
    text: String,
    modifier: Modifier = Modifier,
    color: Color = Color.Black.copy(alpha = 0.55f),
    textColor: Color = Color.White,
) {
    Text(
        text.uppercase(),
        modifier
            .background(color, RoundedCornerShape(6.dp))
            .padding(horizontal = 7.dp, vertical = 3.dp),
        color = textColor,
        style = MaterialTheme.typography.labelSmall,
        maxLines = 1,
    )
}

@Composable
fun PosterCard(
    item: Item,
    width: Dp,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    onFocus: (() -> Unit)? = null,
    cardModifier: Modifier = Modifier,
    showTitle: Boolean = !LocalDimens.current.isTv,
    badge: (@Composable BoxScope.() -> Unit)? = null,
    inContinueWatching: Boolean = false,
) {
    var menu by remember { mutableStateOf(false) }
    if (menu) ItemMenu(item, inContinueWatching) { menu = false }
    Column(modifier.width(width)) {
        FocusCard(onClick = onClick, modifier = cardModifier, onFocus = onFocus, onLongClick = item.menuable { menu = true }) {
            RemoteImage(item.id, item.poster(), width, Modifier.fillMaxWidth().aspectRatio(2f / 3f), contentDescription = item.title)
            ItemServerBadge(item)
            item.progress()?.let { ProgressBar(it, Modifier.align(Alignment.BottomCenter).padding(8.dp)) }
            if (item.user.played) {
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .padding(8.dp)
                        .size(22.dp)
                        .background(Color.Black.copy(alpha = 0.6f), RoundedCornerShape(50)),
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.Check, "Watched", Modifier.size(15.dp), tint = Accent) }
            }
            badge?.invoke(this)
        }
        if (showTitle) {
            Text(
                if (item.kind == ItemKind.EPISODE) item.seriesTitle ?: item.title else item.title,
                Modifier.padding(top = 8.dp),
                style = MaterialTheme.typography.titleSmall,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                if (item.kind == ItemKind.EPISODE && item.seasonNumber != null && item.episodeNumber != null) {
                    "S${item.seasonNumber} · E${item.episodeNumber}"
                } else {
                    item.subtitle()
                },
                style = MaterialTheme.typography.bodySmall,
                color = Muted,
                maxLines = 1,
            )
        }
    }
}

/** 16:9 card for "Continue watching": still, progress, time left. */
@Composable
fun LandscapeCard(
    item: Item,
    width: Dp,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    onFocus: (() -> Unit)? = null,
    cardModifier: Modifier = Modifier,
    inContinueWatching: Boolean = false,
) {
    var menu by remember { mutableStateOf(false) }
    if (menu) ItemMenu(item, inContinueWatching) { menu = false }
    Column(modifier.width(width)) {
        FocusCard(onClick = onClick, modifier = cardModifier, onFocus = onFocus, onLongClick = item.menuable { menu = true }) {
            RemoteImage(item.id, item.thumb(), width, Modifier.fillMaxWidth().aspectRatio(16f / 9f), contentDescription = item.title)
            ItemServerBadge(item)
            Box(
                Modifier
                    .matchParentSize()
                    .background(Brush.verticalGradient(0.5f to Color.Transparent, 1f to Color.Black.copy(alpha = 0.75f))),
            )
            Column(Modifier.align(Alignment.BottomStart).padding(12.dp)) {
                Text(
                    if (item.kind == ItemKind.EPISODE) item.seriesTitle ?: item.title else item.title,
                    style = MaterialTheme.typography.titleSmall,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    listOfNotNull(item.subtitle().takeIf { item.kind == ItemKind.EPISODE }, item.remaining()).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall,
                    color = Muted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                item.progress()?.let { ProgressBar(it, Modifier.padding(top = 8.dp)) }
            }
        }
    }
}

@Composable
fun SectionHeader(
    title: String,
    modifier: Modifier = Modifier,
    action: String? = null,
    onAction: (() -> Unit)? = null,
    subtitle: String? = null,
    onTitle: (() -> Unit)? = null,
) {
    val d = LocalDimens.current
    Row(
        modifier.fillMaxWidth().padding(horizontal = d.gutter),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(
            Modifier
                .weight(1f)
                .then(if (onTitle != null && !d.isTv) Modifier.clickable(onClick = onTitle) else Modifier),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (onTitle != null && !d.isTv) Icon(Icons.ChevronRight, null, Modifier.size(22.dp), tint = Muted)
            }
            subtitle?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 1) }
        }
        if (action != null && onAction != null) {
            FocusCard(onClick = onAction, shape = RoundedCornerShape(50)) {
                Row(Modifier.padding(horizontal = 10.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(action, style = MaterialTheme.typography.labelMedium, color = Muted)
                    Icon(Icons.ChevronRight, null, Modifier.size(18.dp), tint = Muted)
                }
            }
        }
    }
}

/**
 * True while ← is held (key repeats): a fast run back to the start of a row stops on its
 * first card instead of carrying on into the side menu. A new press opens it.
 */
object HeldLeft {
    @Volatile
    var held = false

    /** The last key pressed was ←: the only way into the TV side menu. */
    @Volatile
    var pressed = false
}

/**
 * TV: the last focused card per screen, so coming back (BACK from a detail page) puts the
 * remote where it was instead of nowhere.
 */
object FocusMemory {
    private val last = mutableMapOf<String, String>()

    operator fun get(screen: String): String? = last[screen]

    fun forget(screen: String) {
        last.remove(screen)
    }

    operator fun set(
        screen: String,
        id: String,
    ) {
        last[screen] = id
    }
}

/**
 * TV: records this element as [screen]'s last focused one, and gives it the focus when the
 * screen is shown again (BACK from what it opened). With nothing remembered, [default] takes it.
 */
@Composable
fun Modifier.rememberFocus(
    screen: String,
    id: String,
    default: Boolean = false,
): Modifier {
    if (!LocalDimens.current.isTv) return this
    val remembered = FocusMemory[screen]
    val restore = remembered == id || (remembered == null && default)
    return onFocusChanged { if (it.isFocused) FocusMemory[screen] = id }
        .then(if (restore) Modifier.initialFocus() else Modifier)
}

/**
 * TV: the page's focused element, kept while the side menu has the focus so that closing the
 * menu puts it back exactly there (a focus restorer only remembers one level of a nested list).
 */
class PageFocus {
    var last: FocusRequester? = null
}

/** Provided around the page content (not the menu) on TV. */
val LocalPageFocus = androidx.compose.runtime.staticCompositionLocalOf<PageFocus?> { null }

/** Registers a focusable element with [LocalPageFocus]; goes before its clickable. */
@Composable
fun Modifier.pageFocusTracked(focused: Boolean): Modifier {
    val page = LocalPageFocus.current ?: return this
    val fr = remember { FocusRequester() }
    if (focused) androidx.compose.runtime.SideEffect { page.last = fr }
    return focusRequester(fr)
}

@Composable
fun Rail(
    title: String,
    items: List<Item>,
    onClick: (Item) -> Unit,
    onFocus: ((Item) -> Unit)? = null,
    landscape: Boolean = false,
    focusKey: String? = null,
    defaultFocus: Boolean = false,
    subtitle: String? = null,
    onSeeAll: (() -> Unit)? = null,
    // The Continue watching row (its items' menu can remove them from it).
    continueWatching: Boolean = false,
) {
    if (items.isEmpty()) return
    val d = LocalDimens.current
    Column(Modifier.padding(top = if (d.isTv) 18.dp else 24.dp)) {
        SectionHeader(title, subtitle = subtitle, onTitle = onSeeAll)
        // A refresh can put new titles in front: the row shows them (a lazy row would otherwise
        // keep its old first item in place and hide the new ones to its left).
        val rowState =
            androidx.compose.foundation.lazy
                .rememberLazyListState()
        val head = items.first().id
        LaunchedEffect(head) { if (rowState.firstVisibleItemIndex != 0) rowState.scrollToItem(0) }
        // The focused card left the row (watched, or removed from it with its menu): the card
        // now in its place (or the new last one) takes the focus, instead of nothing.
        val before = remember { arrayOf(items) }
        var moveTo by remember { mutableStateOf<String?>(null) }
        val replacement = remember { FocusRequester() }
        LaunchedEffect(items) {
            val old = before[0]
            before[0] = items
            if (!d.isTv || focusKey == null) return@LaunchedEffect
            val gone = FocusMemory[focusKey]?.takeIf { it.startsWith("$title|") }?.substringAfter('|') ?: return@LaunchedEffect
            if (items.any { it.id == gone }) return@LaunchedEffect
            val at = old.indexOfFirst { it.id == gone }.takeIf { it >= 0 } ?: return@LaunchedEffect
            moveTo = items[at.coerceAtMost(items.lastIndex)].id
        }
        TvScrollSpec(leading = d.gutter) {
            LazyRow(
                // Its own layer: scrolling this row doesn't redraw the rows around it.
                Modifier.graphicsLayer(),
                state = rowState,
                contentPadding = PaddingValues(horizontal = d.gutter, vertical = if (d.isTv) 14.dp else 12.dp),
                horizontalArrangement = Arrangement.spacedBy(d.railSpacing),
            ) {
                items(items, key = { it.id }) { item ->
                    // Rail + item: the same title can sit in two rails (continue watching, next up).
                    val memo = "$title|${item.id}"
                    val focus = {
                        onFocus?.invoke(item)
                        if (focusKey != null) FocusMemory[focusKey] = memo
                    }
                    val remembered = focusKey?.let { FocusMemory[it] }
                    val wantsFocus = d.isTv && (remembered == memo || (remembered == null && defaultFocus && item == items.first()))
                    val cardMod =
                        when {
                            item.id == moveTo -> {
                                LaunchedEffect(Unit) {
                                    runCatching { replacement.requestFocus() }
                                    moveTo = null
                                }
                                Modifier.focusRequester(replacement)
                            }
                            wantsFocus -> Modifier.initialFocus()
                            else -> Modifier
                        }
                    if (landscape) {
                        LandscapeCard(
                            item,
                            d.landscapeWidth,
                            onClick = { onClick(item) },
                            onFocus = focus,
                            cardModifier = cardMod,
                            inContinueWatching = continueWatching,
                        )
                    } else {
                        PosterCard(
                            item,
                            d.posterWidth,
                            onClick = { onClick(item) },
                            onFocus = focus,
                            cardModifier = cardMod,
                            inContinueWatching = continueWatching,
                        )
                    }
                }
            }
        }
    }
}

/**
 * TV scrolling like launchers do: the focused element is brought to a fixed position
 * ([leading] from the start) instead of the minimal scroll, so rows don't end half-cut and the
 * focused card stays at the same spot while the row moves.
 */
@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun TvScrollSpec(
    leading: Dp,
    content: @Composable () -> Unit,
) {
    if (!LocalDimens.current.isTv) {
        content()
        return
    }
    val px = with(androidx.compose.ui.platform.LocalDensity.current) { leading.toPx() }
    val spec =
        remember(px) {
            object : androidx.compose.foundation.gestures.BringIntoViewSpec {
                override fun calculateScrollDistance(
                    offset: Float,
                    size: Float,
                    containerSize: Float,
                ): Float = offset - px
            }
        }
    androidx.compose.runtime.CompositionLocalProvider(
        androidx.compose.foundation.gestures.LocalBringIntoViewSpec provides spec,
        content = content,
    )
}

/** Shimmer-free placeholder blocks, shown only on a first-ever load (never over cached data). */
@Composable
fun SkeletonRail() {
    val d = LocalDimens.current
    Column(Modifier.padding(top = 24.dp, start = d.gutter)) {
        Box(
            Modifier
                .width(140.dp)
                .height(18.dp)
                .clip(RoundedCornerShape(6.dp))
                .background(SurfaceColor),
        )
        Row(Modifier.padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(d.railSpacing)) {
            repeat(6) {
                Box(
                    Modifier
                        .width(d.posterWidth)
                        .aspectRatio(2f / 3f)
                        .clip(RoundedCornerShape(d.radius))
                        .background(SurfaceColor),
                )
            }
        }
    }
}

@Composable
fun ErrorBanner(
    message: String,
    onRetry: () -> Unit,
) {
    val d = LocalDimens.current
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = d.gutter, vertical = 8.dp)
            .clip(RoundedCornerShape(d.radius))
            .background(SurfaceColor)
            .border(1.dp, Outline, RoundedCornerShape(d.radius))
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Box(
            Modifier
                .size(8.dp)
                .clip(RoundedCornerShape(50))
                .background(Danger),
        )
        Text(message, Modifier.weight(1f), color = Muted, style = MaterialTheme.typography.bodyMedium, maxLines = 4)
        ActionButton("Retry", onRetry)
    }
}

// ----- Buttons --------------------------------------------------------------------------------

/**
 * Pill button. Primary: white with black text (the main action). Secondary: translucent glass.
 * On TV only the focused button is white, whatever its style (the clearest focus cue at 3 m); an
 * unfocused primary is a lighter glass, so it never looks focused next to the real focus.
 */
@Composable
fun ActionButton(
    label: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    primary: Boolean = false,
    icon: ImageVector? = null,
    /** The current choice of a group (a season): accent outline, so it never looks focused. */
    selected: Boolean = false,
) {
    val d = LocalDimens.current
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val light = if (d.isTv) focused else primary
    val bg by animateColorAsState(
        when {
            light -> Color.White
            primary -> Color.White.copy(alpha = 0.28f)
            else -> Glass
        },
        tween(140),
        label = "bg",
    )
    val fg =
        if (light) {
            Color.Black
        } else if (selected) {
            Accent
        } else {
            OnSurface
        }
    val scale by animateFloatAsState(if (focused && d.isTv) 1.05f else 1f, tween(140), label = "scale")
    Row(
        modifier
            .pageFocusTracked(focused)
            .graphicsLayer {
                scaleX = scale
                scaleY = scale
            }.clip(RoundedCornerShape(50))
            .background(bg)
            .then(
                if ((focused && !d.isTv) || (selected && !light)) Modifier.border(2.dp, Accent, RoundedCornerShape(50)) else Modifier,
            ).clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = if (icon != null) 18.dp else 20.dp, vertical = if (d.isTv) 11.dp else 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        if (icon != null) Icon(icon, null, Modifier.size(if (d.isTv) 24.dp else 20.dp), tint = fg)
        Text(label, color = fg, style = MaterialTheme.typography.labelLarge, maxLines = 1)
    }
}

/** Round glass button with an icon only (secondary actions on detail pages); on TV the focused one
 * also says what it does, since there is no long-press or hover to ask. */
@Composable
fun IconAction(
    icon: ImageVector,
    contentDescription: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    active: Boolean = false,
) {
    val d = LocalDimens.current
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val light = focused && d.isTv
    val bg by animateColorAsState(if (light) Color.White else Glass, tween(140), label = "bg")
    Row(
        modifier
            .pageFocusTracked(focused)
            .height(if (d.isTv) 52.dp else 46.dp)
            .widthIn(min = if (d.isTv) 52.dp else 46.dp)
            .background(bg, RoundedCornerShape(50))
            .clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = if (light) 16.dp else 0.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
    ) {
        Icon(
            icon,
            contentDescription,
            Modifier.size(22.dp),
            tint =
                if (light) {
                    Color.Black
                } else if (active) {
                    Accent
                } else {
                    OnSurface
                },
        )
        if (light) Text(contentDescription, color = Color.Black, style = MaterialTheme.typography.labelLarge, maxLines = 1)
    }
}

// ----- Ambient background ---------------------------------------------------------------------

/** A soft glow of the artwork's color from the top of the screen, fading into the background. */
fun Modifier.ambientGlow(color: Color?): Modifier = ambientGlow { color }

/**
 * [color] is read while drawing: a state read there (the TV home's hero) repaints the glow
 * without recomposing the screen. The gradient is built once per size and color, not per frame.
 */
fun Modifier.ambientGlow(color: () -> Color?): Modifier =
    drawWithCache {
        val c = color()
        val brush =
            c?.let {
                Brush.radialGradient(
                    0f to it.copy(alpha = 0.38f),
                    1f to Color.Transparent,
                    center = Offset(size.width * 0.3f, 0f),
                    radius = size.maxDimension * 0.75f,
                )
            }
        // The background color itself is painted once by the root (one full-screen fill less
        // per frame: a Chromecast's GPU feels every one).
        onDrawBehind { brush?.let { drawRect(it) } }
    }

/**
 * Fades the content itself to transparent towards the bottom (and the left edge on TV), so
 * artwork melts into whatever is behind it (the ambient glow) without a visible seam.
 */
fun Modifier.fadeEdges(
    bottomFrom: Float = 0.55f,
    leftTo: Float = 0f,
): Modifier =
    graphicsLayer { compositingStrategy = androidx.compose.ui.graphics.CompositingStrategy.Offscreen }
        .drawWithCache {
            // Built once per size, not on every frame of a cross-fade.
            val bottom = Brush.verticalGradient(bottomFrom to Color.Black, 1f to Color.Transparent)
            val left = if (leftTo > 0f) Brush.horizontalGradient(0f to Color.Transparent, leftTo to Color.Black) else null
            onDrawWithContent {
                drawContent()
                drawRect(bottom, blendMode = androidx.compose.ui.graphics.BlendMode.DstIn)
                left?.let { drawRect(it, blendMode = androidx.compose.ui.graphics.BlendMode.DstIn) }
            }
        }

// ----- Focus helpers --------------------------------------------------------------------------

/**
 * Lets D-pad up/down leave a text field (Compose text fields otherwise keep focus and move the
 * cursor), which is required to reach the buttons below a field with a remote.
 */
@Composable
fun Modifier.dpadEscape(): Modifier {
    val fm = LocalFocusManager.current
    return onPreviewKeyEvent {
        if (it.type != KeyEventType.KeyDown) return@onPreviewKeyEvent false
        when (it.key) {
            Key.DirectionDown -> fm.moveFocus(FocusDirection.Down)
            Key.DirectionUp -> fm.moveFocus(FocusDirection.Up)
            else -> false
        }
    }
}

/** Per-screen flag for [initialFocus] (provided by the navigation host for each screen). */
val LocalInitialFocusDone = androidx.compose.runtime.staticCompositionLocalOf<androidx.compose.runtime.MutableState<Boolean>?> { null }

/** Requests focus once on first composition (TV: put the remote on the main action). */
@Composable
fun Modifier.initialFocus(enabled: Boolean = LocalDimens.current.isTv): Modifier {
    if (!enabled) return this
    val fr = remember { FocusRequester() }
    // Once per screen: an element that leaves and comes back (data reloading) must not steal
    // the focus from wherever the user moved it since.
    val owner = LocalInitialFocusDone.current
    LaunchedEffect(Unit) {
        if (owner?.value == true) return@LaunchedEffect
        if (runCatching { fr.requestFocus() }.getOrDefault(false)) owner?.value = true
    }
    return focusRequester(fr)
}

/**
 * Takes the focus whenever it appears, even after the screen's first focus: for what replaces
 * the focused element (a Request button that becomes the request panel, and back).
 */
@Composable
fun Modifier.focusOnAppear(enabled: Boolean = LocalDimens.current.isTv): Modifier {
    if (!enabled) return this
    val fr = remember { FocusRequester() }
    val owner = LocalInitialFocusDone.current
    LaunchedEffect(Unit) { if (runCatching { fr.requestFocus() }.getOrDefault(false)) owner?.value = true }
    return focusRequester(fr)
}

/** Set where several servers are mixed (combined home, search): cards show which server. */
val LocalServerBadges = androidx.compose.runtime.staticCompositionLocalOf { false }

/**
 * A small mark of the server an account belongs to: a chevron in Plex's yellow, a triangle in
 * Jellyfin's purple-to-blue (simple shapes, not the brands' logo files).
 */
@Composable
fun ServerBadge(
    accountId: String,
    modifier: Modifier = Modifier,
    diameter: Dp = 22.dp,
) {
    val plex = accountId.startsWith("plex:")
    Box(
        modifier
            .size(diameter)
            .background(Color.Black.copy(alpha = 0.72f), RoundedCornerShape(50))
            .padding(diameter * 0.23f)
            // Paths built once per size, not on every frame a card moves.
            .drawWithCache {
                val w = size.width
                val h = size.height
                if (plex) {
                    val chevron =
                        androidx.compose.ui.graphics.Path().apply {
                            moveTo(w * 0.22f, 0f)
                            lineTo(w * 0.52f, 0f)
                            lineTo(w * 0.86f, h / 2)
                            lineTo(w * 0.52f, h)
                            lineTo(w * 0.22f, h)
                            lineTo(w * 0.56f, h / 2)
                            close()
                        }
                    onDrawBehind { drawPath(chevron, Color(0xFFE5A00D)) }
                } else {
                    val outer =
                        androidx.compose.ui.graphics.Path().apply {
                            moveTo(w / 2, 0f)
                            lineTo(w, h)
                            lineTo(0f, h)
                            close()
                        }
                    val brush = Brush.linearGradient(listOf(Color(0xFFAA5CC3), Color(0xFF00A4DC)), Offset(0f, 0f), Offset(w, h))
                    val inner =
                        androidx.compose.ui.graphics.Path().apply {
                            moveTo(w / 2, h * 0.42f)
                            lineTo(w * 0.72f, h * 0.86f)
                            lineTo(w * 0.28f, h * 0.86f)
                            close()
                        }
                    onDrawBehind {
                        drawPath(outer, brush)
                        drawPath(inner, Color.Black.copy(alpha = 0.72f))
                    }
                }
            },
    )
}

/** The server badge of an item, when the screen mixes servers. */
@Composable
private fun BoxScope.ItemServerBadge(item: Item) {
    if (!LocalServerBadges.current) return
    val owner = LocalItemOwners.current[item.id] ?: return
    ServerBadge(owner, Modifier.align(Alignment.TopStart).padding(8.dp))
}
