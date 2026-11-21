// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.ForeverPerson
import be.alwyapps.watch.core.keep.ForeverStats
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.SurfaceHigh
import be.alwyapps.watch.ui.UrlImage
import be.alwyapps.watch.ui.Value

private enum class Spread(
    val label: String,
) {
    ALL("All"),
    SHARED("Kept by several"),
    ALONE("Kept by one person"),
}

/**
 * Admins: who keeps what forever, and above all how much of it someone else keeps too (titles
 * kept by one person alone are the ones that cost space for a single viewer).
 */
@Composable
fun KeepForeverScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    var stats by remember { mutableStateOf<ForeverStats?>(null) }
    var message by remember { mutableStateOf<String?>(null) }
    var person by remember { mutableStateOf<String?>(null) }
    var spread by remember { mutableStateOf(Spread.ALL) }
    LaunchedEffect(Unit) {
        try {
            stats = core.keepAdminForever()
        } catch (e: CoreException) {
            message = e.message
        }
    }
    val s = stats
    val names = s?.people.orEmpty().associate { it.userKey to it.name }
    val shown =
        s
            ?.items
            .orEmpty()
            .filter { person == null || person in it.keepers }
            .filter {
                when (spread) {
                    Spread.ALL -> true
                    Spread.SHARED -> it.keepers.size > 1
                    Spread.ALONE -> it.keepers.size == 1
                }
            }
    LazyColumn(
        Modifier.fillMaxSize(),
        contentPadding = PaddingValues(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(22.dp),
    ) {
        item {
            PageTitle(
                "Kept forever",
                s?.let { "${it.titles} titles · ${it.shared} kept by several people · ${it.titles - it.shared} by one person" }
                    ?: "Loading…",
            )
        }
        message?.let { item { Text(it, color = Accent) } }
        if (s != null && s.people.isEmpty()) {
            item { Text("Nobody keeps anything forever yet.", color = Muted) }
        }
        if (s != null && s.people.isNotEmpty()) {
            item {
                Group("People", footer = "Shared: someone else keeps it forever too. Choose a person to see their titles.") {
                    s.people.forEachIndexed { i, p ->
                        if (i > 0) Divider()
                        PersonRow(p, selected = person == p.userKey) {
                            person = if (person == p.userKey) null else p.userKey
                        }
                    }
                }
            }
            item {
                LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    items(Spread.entries.toList()) { v ->
                        ActionButton(v.label, { spread = v }, selected = spread == v)
                    }
                    person?.let { k ->
                        item { ActionButton("${names[k] ?: k} ✕", { person = null }, selected = true) }
                    }
                }
            }
            item {
                Group(
                    listOfNotNull(person?.let { names[it] }, "${shown.size} titles").joinToString(" · "),
                ) {
                    if (shown.isEmpty()) {
                        Text("Nothing here.", Modifier.padding(16.dp), color = Muted)
                    }
                    shown.forEachIndexed { i, t ->
                        if (i > 0) Divider()
                        val others = t.keepers.filter { it != person }.map { names[it] ?: it }
                        ListRow(
                            t.title + (t.year?.let { " ($it)" } ?: ""),
                            subtitle =
                                when {
                                    person != null && others.isEmpty() -> "Only them"
                                    person != null -> "Also " + others.joinToString(", ")
                                    else -> t.keepers.joinToString(", ") { names[it] ?: it }
                                },
                            leading = {
                                Box(
                                    Modifier
                                        .width(40.dp)
                                        .aspectRatio(2f / 3f)
                                        .clip(RoundedCornerShape(4.dp))
                                        .background(SurfaceHigh),
                                ) {
                                    UrlImage(t.posterUrl, 40.dp, Modifier.fillMaxSize())
                                }
                            },
                            trailing = {
                                val n = t.keepers.size
                                Value(if (n == 1) "1 person" else "$n people")
                            },
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun PersonRow(
    p: ForeverPerson,
    selected: Boolean,
    onClick: () -> Unit,
) {
    Column {
        ListRow(
            p.name,
            subtitle =
                listOfNotNull(
                    "${p.shared} shared · ${p.alone} only them",
                    p.overlaps
                        .take(3)
                        .takeIf { it.isNotEmpty() }
                        ?.joinToString(", ", prefix = "most with ") { "${it.name} (${it.both})" },
                ).joinToString(" · "),
            icon = Icons.Person,
            iconTint = if (selected) Accent else be.alwyapps.watch.ui.OnSurface,
            onClick = onClick,
            trailing = { Value("${p.count}") },
        )
        // Shared (accent) against alone (muted) at a glance.
        if (p.count > 0u) {
            Row(
                Modifier
                    .padding(start = 16.dp, end = 16.dp, bottom = 12.dp)
                    .fillMaxWidth()
                    .height(6.dp)
                    .clip(RoundedCornerShape(3.dp))
                    .background(SurfaceHigh),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                if (p.shared > 0u) {
                    Box(Modifier.weight(p.shared.toFloat()).fillMaxHeight().background(Accent))
                }
                if (p.alone > 0u) {
                    Box(Modifier.weight(p.alone.toFloat()).fillMaxHeight().background(Muted.copy(alpha = 0.5f)))
                }
            }
        }
    }
}
