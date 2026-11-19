// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
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
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch

/** A top-level destination shown in the bottom bar (phone) or the side rail (TV). */
data class Tab(
    val route: Route,
    val label: String,
    val icon: ImageVector,
    val selectedIcon: ImageVector = icon,
)

/** Which routes are top-level (bars visible); everything else is full screen. */
fun Route.isTopLevel(): Boolean =
    when (this) {
        Route.Home, Route.Search, Route.LeavingSoon, Route.Requests, Route.Downloads, Route.Settings -> true
        is Route.Library -> true
        else -> false
    }

@Composable
fun tabs(): List<Tab> {
    val core = LocalCore.current
    val d = LocalDimens.current
    val out = mutableListOf(Tab(Route.Home, "Home", Icons.Home, Icons.HomeFilled), Tab(Route.Search, "Search", Icons.Search))
    if (d.isTv) {
        // Libraries as destinations (from the cached home: no network).
        val account = LocalAccount.current
        val libs = androidx.compose.runtime.remember(account) { runCatching { core.cachedHome(account)?.libraries }.getOrNull().orEmpty() }
        for (lib in libs.filter { it.kind != be.alwyapps.watch.core.media.LibraryKind.OTHER }) {
            val icon = if (lib.kind == be.alwyapps.watch.core.media.LibraryKind.SHOWS) Icons.Tv else Icons.Movie
            out += Tab(Route.Library(lib.id, lib.title), lib.title, icon)
        }
    }
    if (core.keepActive()) out += Tab(Route.LeavingSoon, "Leaving", Icons.Hourglass)
    // Phones: five slots at most; with "leaving soon" connected, requests live in Search.
    if (core.seerrActive() && (d.isTv || !core.keepActive())) out += Tab(Route.Requests, "Requests", Icons.Add)
    if (!d.isTv) out += Tab(Route.Downloads, "Downloads", Icons.Download)
    out += Tab(Route.Settings, "Settings", Icons.Settings)
    return out
}

/** Bottom bar on phones and tablets, collapsible side rail on TV. */
@Composable
fun AppScaffold(content: @Composable () -> Unit) {
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val showBars = nav.current.isTopLevel()
    // Artwork screens draw under the status bar; the others keep clear of it.
    val edgeToEdge = nav.current == Route.Home || nav.current is Route.Detail || nav.current == Route.AddServer
    if (!showBars) {
        Box(if (edgeToEdge) Modifier.fillMaxSize().imePadding() else Modifier.fillMaxSize().safeDrawingPadding()) { content() }
        return
    }
    val tabs = tabs()
    val selected = tabs.firstOrNull { it.route == nav.current }?.route ?: tabs.first().route
    if (d.isTv) {
        Row(Modifier.fillMaxSize()) {
            SideRail(tabs, selected) { nav.reset(it) }
            Box(Modifier.weight(1f).fillMaxHeight().then(if (edgeToEdge) Modifier else Modifier.padding(top = 24.dp))) { content() }
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
    onSelect: (Route) -> Unit,
) {
    var expanded by remember { mutableStateOf(false) }
    val fm = androidx.compose.ui.platform.LocalFocusManager.current
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    // After choosing a destination, hand the focus to its content (and fold the rail).
    val select: (Route) -> Unit = { route ->
        onSelect(route)
        scope.launch {
            kotlinx.coroutines.delay(60)
            fm.moveFocus(androidx.compose.ui.focus.FocusDirection.Right)
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
            ).onFocusChanged { expanded = it.hasFocus }
            .focusGroup()
            .padding(horizontal = 16.dp, vertical = 32.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterVertically),
    ) {
        for (t in tabs) RailItem(t, t.route == selected, expanded) { select(t.route) }
        Spacer(Modifier.height(32.dp))
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
