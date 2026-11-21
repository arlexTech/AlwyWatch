// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp

/** Page title used by list-style screens (settings, downloads, requests…). */
@Composable
fun PageTitle(
    title: String,
    subtitle: String? = null,
) {
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    // Phones: screens opened from the top bar (search, downloads, settings…) get a back arrow.
    val back = !d.isTv && nav.stack.size > 1
    Row(Modifier.padding(bottom = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        if (back) IconAction(Icons.Back, "Back", { nav.pop() })
        Column {
            Text(title, style = MaterialTheme.typography.displaySmall)
            subtitle?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = Muted) }
        }
    }
}

/** A titled group of rows on one rounded card (iOS / One UI settings style). */
@Composable
fun Group(
    title: String?,
    modifier: Modifier = Modifier,
    footer: String? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val d = LocalDimens.current
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        title?.let {
            Text(it.uppercase(), Modifier.padding(start = 4.dp), style = MaterialTheme.typography.labelSmall, color = Subtle)
        }
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(d.radius + 4.dp))
                .background(SurfaceColor)
                .border(1.dp, Outline, RoundedCornerShape(d.radius + 4.dp)),
            content = content,
        )
        footer?.let { Text(it, Modifier.padding(horizontal = 4.dp), style = MaterialTheme.typography.bodySmall, color = Subtle) }
    }
}

/**
 * One row of a [Group]: optional icon, title, subtitle and a trailing element. The whole row is
 * the click target (and focus target on TV) when [onClick] is set.
 */
@Composable
fun ListRow(
    title: String,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    icon: ImageVector? = null,
    iconTint: Color = OnSurface,
    onClick: (() -> Unit)? = null,
    trailing: (@Composable () -> Unit)? = null,
    leading: (@Composable () -> Unit)? = null,
) {
    val d = LocalDimens.current
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val bg by animateColorAsState(if (focused) Color.White.copy(alpha = if (d.isTv) 0.16f else 0.08f) else Color.Transparent, tween(120), label = "row")
    Row(
        modifier
            .fillMaxWidth()
            .background(bg)
            .then(if (onClick != null) Modifier.clickable(interactionSource = interaction, indication = null, onClick = onClick) else Modifier)
            .padding(horizontal = 16.dp, vertical = if (d.isTv) 16.dp else 14.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        leading?.invoke()
        if (icon != null && leading == null) {
            Box(
                Modifier
                    .size(if (d.isTv) 40.dp else 34.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(SurfaceHigh),
                contentAlignment = Alignment.Center,
            ) { Icon(icon, null, Modifier.size(20.dp), tint = iconTint) }
        }
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
            subtitle?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 2, overflow = TextOverflow.Ellipsis) }
        }
        trailing?.invoke()
    }
}

@Composable
fun Divider() {
    Box(
        Modifier
            .fillMaxWidth()
            .padding(start = 16.dp)
            .height(1.dp)
            .background(Outline),
    )
}

/** Pill switch (display only: the row it sits in handles the click and focus). */
@Composable
fun Switch(checked: Boolean) {
    val track by animateColorAsState(if (checked) Accent else SurfaceHigh, tween(150), label = "track")
    val x by animateDpAsState(if (checked) 20.dp else 2.dp, tween(150), label = "thumb")
    Box(
        Modifier
            .width(44.dp)
            .height(26.dp)
            .clip(RoundedCornerShape(50))
            .background(track),
    ) {
        Box(
            Modifier
                .offset(x = x, y = 2.dp)
                .size(22.dp)
                .clip(RoundedCornerShape(50))
                .background(Color.White),
        )
    }
}

@Composable
fun ToggleRow(
    title: String,
    checked: Boolean,
    onChange: (Boolean) -> Unit,
    subtitle: String? = null,
    icon: ImageVector? = null,
) = ListRow(title, subtitle = subtitle, icon = icon, onClick = { onChange(!checked) }, trailing = { Switch(checked) })

/** Trailing text for value rows ("In use", "4 MB"…). */
@Composable
fun Value(text: String) = Text(text, style = MaterialTheme.typography.bodyMedium, color = Muted)

/** Trailing chevron for rows that open something. */
@Composable
fun Chevron() = Icon(Icons.ChevronRight, null, Modifier.size(22.dp), tint = Subtle)

/** "Are you sure?" before deleting something. */
@Composable
fun ConfirmDialog(
    title: String,
    text: String,
    confirm: String = "Delete",
    onConfirm: () -> Unit,
    onDismiss: () -> Unit,
) {
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(text, color = Muted) },
        confirmButton = {
            ActionButton(confirm, {
                onConfirm()
                onDismiss()
            }, primary = true)
        },
        dismissButton = { ActionButton("Cancel", onDismiss) },
        containerColor = Background,
    )
}

/** A setting with a few values: the row shows the current one, a list opens to pick another. */
@Composable
fun ChoiceRow(
    title: String,
    options: List<Pair<String, String>>,
    value: String,
    onPick: (String) -> Unit,
    subtitle: String? = null,
    icon: ImageVector? = null,
    /** A mark before each value (a server's Plex/Jellyfin badge), in the row and the list. */
    mark: (@Composable (String) -> Unit)? = null,
) {
    var open by androidx.compose.runtime.saveable
        .rememberSaveable { androidx.compose.runtime.mutableStateOf(false) }
    val label = options.firstOrNull { it.first == value }?.second ?: options.firstOrNull()?.second.orEmpty()
    ListRow(title, subtitle = subtitle, icon = icon, onClick = { open = true }, trailing = {
        androidx.compose.foundation.layout.Row(
            horizontalArrangement =
                androidx.compose.foundation.layout.Arrangement
                    .spacedBy(8.dp),
            verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
        ) {
            mark?.invoke(value)
            Value(label)
        }
    })
    if (open) {
        ChoiceDialog(title, options, value, onPick = {
            onPick(it)
            open = false
        }, onDismiss = { open = false }, mark = mark)
    }
}

/** A list of values to pick one from (settings, audio/subtitle tracks, versions). */
@Composable
fun ChoiceDialog(
    title: String,
    options: List<Pair<String, String>>,
    value: String?,
    onPick: (String) -> Unit,
    onDismiss: () -> Unit,
    mark: (@Composable (String) -> Unit)? = null,
) {
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            androidx.compose.foundation.lazy.LazyColumn {
                items(options.size) { n ->
                    val (key, text) = options[n]
                    val selected = key == value
                    val focus =
                        remember {
                            androidx.compose.ui.focus
                                .FocusRequester()
                        }
                    if (selected) {
                        androidx.compose.runtime.LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
                    }
                    ListRow(
                        text,
                        Modifier.focusRequester(focus),
                        onClick = { onPick(key) },
                        trailing = if (selected) ({ Icon(Icons.Check, null, Modifier.size(20.dp), tint = Accent) }) else null,
                        leading = mark?.let { m -> { m(key) } },
                    )
                }
            }
        },
        confirmButton = { ActionButton("Close", onDismiss) },
        containerColor = Background,
    )
}
