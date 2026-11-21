// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusGroup
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusProperties
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.focusRestorer
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.services
import kotlinx.coroutines.launch

/** A top-level destination shown in the bottom bar (phone) or the side rail (TV). */
data class Tab(
    val route: Route,
    val label: String,
    val icon: ImageVector,
    val selectedIcon: ImageVector = icon,
)

/**
 * Which routes are top-level (bars visible); everything else is full screen. On phones,
 * search, downloads and settings open from the home top bar (like Plex), full screen.
 */
fun Route.isTopLevel(rail: Boolean): Boolean =
    when (this) {
        Route.Home, Route.LeavingSoon, Route.Together -> true
        Route.Search, Route.Downloads, Route.Settings, Route.Watchlist -> rail
        is Route.Library -> true
        else -> false
    }

/** TV and wide screens navigate with a side rail; phones with the top bar and a bottom bar. */
@Composable
fun usesRail(): Boolean = LocalDimens.current.isTv || androidx.compose.ui.platform.LocalConfiguration.current.screenWidthDp >= 840

@Composable
fun tabs(): List<Tab> {
    val core = LocalCore.current
    val d = LocalDimens.current
    val rail = usesRail()
    val out = mutableListOf(Tab(Route.Home, "Home", Icons.Home, Icons.HomeFilled))
    if (rail) out += Tab(Route.Watchlist, "My list", Icons.Bookmark, Icons.BookmarkFilled)
    if (rail) out += Tab(Route.Search, "Search", Icons.Search)
    if (rail) {
        // Libraries as destinations (from the cached home: no network).
        val account = LocalAccount.current
        val libs = androidx.compose.runtime.remember(account) { runCatching { core.cachedHome(account)?.libraries }.getOrNull().orEmpty() }
        val prefs = UiPrefs(core, d.isTv)
        for (lib in prefs.arrange(libs.filter { it.kind != be.alwyapps.watch.core.media.LibraryKind.OTHER }, { it.id })) {
            val icon = if (lib.kind == be.alwyapps.watch.core.media.LibraryKind.SHOWS) Icons.Tv else Icons.Movie
            out += Tab(Route.Library(lib.id, lib.title), lib.title, icon)
        }
    }
    if (core.keepActive()) out += Tab(Route.LeavingSoon, "Leaving", Icons.Hourglass)
    // Watch together goes through the AlwyWatch server.
    if (core.keepActive()) out += Tab(Route.Together, "Together", Icons.Group)
    if (rail && !d.isTv) out += Tab(Route.Downloads, "Downloads", Icons.Download)
    if (rail) out += Tab(Route.Settings, "Settings", Icons.Settings)
    return out
}

/** Bottom bar on phones and tablets, collapsible side rail on TV. */
@Composable
fun AppScaffold(content: @Composable () -> Unit) {
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val rail = usesRail()
    val showBars = nav.current.isTopLevel(rail)
    // Artwork screens draw under the status bar; the others keep clear of it.
    val edgeToEdge = nav.current == Route.Home || nav.current is Route.Detail || nav.current == Route.AddServer
    if (!showBars) {
        Box(if (edgeToEdge) Modifier.fillMaxSize().imePadding() else Modifier.fillMaxSize().safeDrawingPadding()) { content() }
        return
    }
    val tabs = tabs()
    val selected = tabs.firstOrNull { it.route == nav.current }?.route ?: tabs.first().route
    // Phone with nothing but Home to switch to: no bottom bar at all.
    if (!rail && tabs.size < 2) {
        Box(if (edgeToEdge) Modifier.fillMaxSize().navigationBarsPadding().imePadding() else Modifier.fillMaxSize().safeDrawingPadding()) { content() }
        return
    }
    // Wide touch screens (tablets in landscape, foldables): a side rail like TV, always labelled.
    if (d.isTv) {
        // Like Plex: the menu floats over the content as a small pill of icons.
        Box(Modifier.fillMaxSize()) {
            // Leaving the menu restores the focus where it was in the page.
            val page =
                remember {
                    androidx.compose.ui.focus
                        .FocusRequester()
                }
            Box(
                Modifier
                    .fillMaxSize()
                    .focusRequester(page)
                    .focusRestorer()
                    .then(if (edgeToEdge) Modifier else Modifier.padding(top = 24.dp)),
            ) { content() }
            TvMenu(tabs, selected, page) { nav.reset(it) }
            if (UiPrefs(LocalCore.current, true).clock) TvClock(Modifier.align(Alignment.TopEnd))
        }
    } else if (rail) {
        Row(Modifier.fillMaxSize()) {
            SideRail(tabs, selected, alwaysExpanded = !d.isTv) { nav.reset(it) }
            val inner =
                when {
                    d.isTv -> if (edgeToEdge) Modifier else Modifier.padding(top = 24.dp)
                    edgeToEdge -> Modifier.imePadding()
                    else -> Modifier.statusBarsPadding().navigationBarsPadding().imePadding()
                }
            Box(Modifier.weight(1f).fillMaxHeight().then(inner)) { content() }
        }
    } else {
        Box(Modifier.fillMaxSize()) {
            Box(
                Modifier
                    .fillMaxSize()
                    .then(if (edgeToEdge) Modifier else Modifier.statusBarsPadding())
                    .navigationBarsPadding()
                    .padding(bottom = 68.dp)
                    // Keyboard open: content shrinks above it (fields and their buttons stay visible).
                    .imePadding(),
            ) { content() }
            BottomBar(tabs, selected, Modifier.align(Alignment.BottomCenter)) { nav.reset(it) }
        }
    }
}

@Composable
private fun BottomBar(
    tabs: List<Tab>,
    selected: Route,
    modifier: Modifier = Modifier,
    onSelect: (Route) -> Unit,
) {
    Column(
        modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(0f to Background.copy(alpha = 0.85f), 1f to Background))
            .navigationBarsPadding(),
    ) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(1.dp)
                .background(Outline),
        )
        Row(
            Modifier.fillMaxWidth().height(68.dp),
            horizontalArrangement = Arrangement.SpaceEvenly,
            verticalAlignment = Alignment.CenterVertically,
        ) {
            for (t in tabs) {
                val on = t.route == selected
                val tint by animateColorAsState(if (on) Color.White else Subtle, tween(150), label = "tint")
                Column(
                    Modifier
                        .weight(1f)
                        .clip(RoundedCornerShape(16.dp))
                        .clickable { onSelect(t.route) }
                        .padding(vertical = 6.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Box(
                        Modifier
                            .clip(RoundedCornerShape(50))
                            .background(if (on) SurfaceHigh else Color.Transparent)
                            .padding(horizontal = 16.dp, vertical = 4.dp),
                    ) { Icon(if (on) t.selectedIcon else t.icon, t.label, Modifier.size(24.dp), tint = tint) }
                    Text(t.label, style = MaterialTheme.typography.labelMedium, color = tint, modifier = Modifier.padding(top = 3.dp))
                }
            }
        }
    }
}

/**
 * TV navigation: icons only while the content has focus; when the remote moves left into the
 * rail it widens and shows labels (like Google TV).
 */
@Composable
private fun SideRail(
    tabs: List<Tab>,
    selected: Route,
    alwaysExpanded: Boolean = false,
    onSelect: (Route) -> Unit,
) {
    var focusExpanded by remember { mutableStateOf(false) }
    val expanded = alwaysExpanded || focusExpanded
    val fm = androidx.compose.ui.platform.LocalFocusManager.current
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    // After choosing a destination, hand the focus to its content (and fold the rail).
    val select: (Route) -> Unit = { route ->
        onSelect(route)
        if (!alwaysExpanded) {
            scope.launch {
                kotlinx.coroutines.delay(60)
                fm.moveFocus(androidx.compose.ui.focus.FocusDirection.Right)
            }
        }
    }
    val width by animateDpAsState(if (expanded) 220.dp else 88.dp, tween(180), label = "rail")
    Column(
        Modifier
            .width(width)
            .fillMaxHeight()
            .background(
                if (expanded) {
                    Brush.horizontalGradient(0f to Color.Black.copy(alpha = 0.92f), 1f to Color.Black.copy(alpha = 0.6f))
                } else {
                    Brush.horizontalGradient(0f to Color.Black.copy(alpha = 0.5f), 1f to Color.Transparent)
                },
            ).then(if (alwaysExpanded) Modifier.systemBarsPadding() else Modifier)
            .onFocusChanged { focusExpanded = it.hasFocus }
            .focusGroup()
            .padding(horizontal = 16.dp, vertical = 32.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterVertically),
    ) {
        for (t in tabs) RailItem(t, t.route == selected, expanded) { select(t.route) }
        Spacer(Modifier.height(32.dp))
    }
}

/** The time, top right on TV (like Plex), in the device's 12/24-hour format. */
@Composable
private fun TvClock(modifier: Modifier) {
    val ctx = androidx.compose.ui.platform.LocalContext.current
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    androidx.compose.runtime.LaunchedEffect(Unit) {
        while (true) {
            now = System.currentTimeMillis()
            kotlinx.coroutines.delay(60_000 - now % 60_000)
        }
    }
    Text(
        android.text.format.DateFormat
            .getTimeFormat(ctx)
            .format(java.util.Date(now)),
        modifier
            .padding(top = 20.dp, end = 32.dp)
            .clip(RoundedCornerShape(50))
            .background(Color.Black.copy(alpha = 0.45f))
            .padding(horizontal = 14.dp, vertical = 6.dp),
        style = MaterialTheme.typography.labelLarge,
        color = Color.White.copy(alpha = 0.9f),
    )
}

/**
 * TV menu, between Plex's (a tiny pill) and a full drawer: a small pill of icons at the left
 * edge; focused, it opens as a card over the dimmed content, the current page focused first.
 * Right or Back returns to where the focus was in the page.
 */
@Composable
private fun TvMenu(
    tabs: List<Tab>,
    selected: Route,
    content: androidx.compose.ui.focus.FocusRequester,
    onSelect: (Route) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val current =
        remember {
            androidx.compose.ui.focus
                .FocusRequester()
        }
    val leave = { runCatching { content.requestFocus() } }
    val select: (Route) -> Unit = { route ->
        onSelect(route)
        scope.launch {
            kotlinx.coroutines.delay(60)
            leave()
        }
    }
    val shade by androidx.compose.animation.core
        .animateFloatAsState(if (open) 1f else 0f, tween(160), label = "menu")
    // The page dims behind the open menu.
    Box(Modifier.fillMaxSize().graphicsLayer { alpha = shade }.background(Color.Black.copy(alpha = 0.6f)))
    Box(
        Modifier
            .padding(start = 16.dp, top = 16.dp, bottom = 16.dp)
            .fillMaxHeight()
            .width(184.dp)
            .graphicsLayer { alpha = shade }
            .clip(RoundedCornerShape(16.dp))
            .background(Color.Black)
            .border(1.dp, Color.White.copy(alpha = 0.12f), RoundedCornerShape(16.dp)),
    )
    Column(
        Modifier
            .fillMaxHeight()
            .padding(start = if (open) 24.dp else 12.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        Column(
            Modifier
                .clip(RoundedCornerShape(22.dp))
                .background(if (open) Color.Transparent else Color.Black.copy(alpha = 0.55f))
                .onFocusChanged { open = it.hasFocus }
                .focusProperties { onEnter = { if (HeldLeft.held) cancelFocusChange() else runCatching { current.requestFocus() } } }
                .onPreviewKeyEvent {
                    val k = it.nativeKeyEvent
                    val out = k.keyCode == android.view.KeyEvent.KEYCODE_DPAD_RIGHT || k.keyCode == android.view.KeyEvent.KEYCODE_BACK
                    if (out && k.action == android.view.KeyEvent.ACTION_DOWN) leave()
                    out
                }.focusGroup()
                .padding(4.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            // Several profiles on this device: who is watching, at the top (like Plex).
            val ctx = androidx.compose.ui.platform.LocalContext.current
            val services = ctx.services
            val me =
                remember {
                    services.profiles
                        .list()
                        .takeIf { it.size > 1 }
                        ?.firstOrNull { it.id == services.profile }
                }
            if (me != null) {
                TvMenuItem(
                    Tab(Route.Settings, "${me.name} \u00b7 Switch", Icons.Person),
                    selected = false,
                    open = open,
                    modifier = Modifier,
                ) {
                    be.alwyapps.watch.ui.screens.ProfileSession.open = false
                    be.alwyapps.watch.ui.screens.ProfileSession.picker = true
                    (ctx as? android.app.Activity)?.recreate()
                }
            }
            for (t in tabs) {
                val on = t.route == selected
                TvMenuItem(t, on, open, if (on) Modifier.focusRequester(current) else Modifier) { select(t.route) }
            }
        }
    }
}

@Composable
private fun TvMenuItem(
    tab: Tab,
    selected: Boolean,
    open: Boolean,
    modifier: Modifier,
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val fg =
        if (focused) {
            Color.Black
        } else if (selected) {
            Color.White
        } else {
            Subtle
        }
    Row(
        modifier
            .then(if (open) Modifier.width(160.dp) else Modifier)
            .clip(RoundedCornerShape(50))
            .background(if (focused) Color.White else Color.Transparent)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = 9.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Icon(if (selected) tab.selectedIcon else tab.icon, tab.label, Modifier.size(20.dp), tint = fg)
        if (open) {
            Text(
                tab.label,
                style = MaterialTheme.typography.bodySmall,
                fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold,
                color = fg,
                maxLines = 1,
            )
        }
    }
}

@Composable
private fun RailItem(
    tab: Tab,
    selected: Boolean,
    expanded: Boolean,
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val bg = if (focused) Color.White else Color.Transparent
    val fg =
        if (focused) {
            Color.Black
        } else if (selected) {
            Color.White
        } else {
            Subtle
        }
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(50))
            .background(bg)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Icon(if (selected) tab.selectedIcon else tab.icon, tab.label, Modifier.size(28.dp), tint = fg)
        if (expanded) Text(tab.label, style = MaterialTheme.typography.titleSmall, color = fg, maxLines = 1)
    }
}
