// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.HistoryCopyResult
import be.alwyapps.watch.core.HistoryPlan
import be.alwyapps.watch.core.media.ServerKind
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.ChoiceRow
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.initialFocus
import kotlinx.coroutines.launch

/**
 * Copy watch history from one server to another (Plex → Jellyfin when moving): watched titles
 * and resume points. Checks first and shows what will change; writes only on "Copy". Only adds:
 * nothing is unmarked, no resume point goes backwards.
 */
@Composable
fun HistoryCopyScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    val accounts = remember { runCatching { core.accounts() }.getOrDefault(emptyList()) }
    val options = accounts.map { it.id to "${it.serverName} · ${it.userName}" }
    // Plex to Jellyfin by default: the usual move.
    var from by remember {
        mutableStateOf(accounts.firstOrNull { it.kind == ServerKind.PLEX }?.id ?: accounts.firstOrNull()?.id.orEmpty())
    }
    var to by remember {
        mutableStateOf(accounts.firstOrNull { it.kind == ServerKind.JELLYFIN && it.id != from }?.id ?: accounts.firstOrNull { it.id != from }?.id.orEmpty())
    }
    var plan by remember { mutableStateOf<HistoryPlan?>(null) }
    var result by remember { mutableStateOf<HistoryCopyResult?>(null) }
    var busy by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var showMissing by remember { mutableStateOf(false) }

    fun reset() {
        plan = null
        result = null
        error = null
    }

    fun check() {
        if (busy != null || from == to) return
        reset()
        busy = "Reading both histories… this can take a minute on big libraries."
        scope.launch {
            try {
                plan = core.historyCopyPlan(from, to)
            } catch (e: CoreException) {
                error = e.message
            } finally {
                busy = null
            }
        }
    }

    fun copy() {
        val p = plan ?: return
        if (busy != null) return
        busy = "Copying ${p.changes.size} titles…"
        scope.launch {
            try {
                result = core.historyCopyApply(to, p.changes)
                plan = null
            } catch (e: CoreException) {
                error = e.message
            } finally {
                busy = null
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 24.dp)
            .widthIn(max = 640.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        PageTitle("Copy watch history")
        Text(
            "Marks what you watched on one server as watched on the other, and copies where you " +
                "stopped. It only adds: nothing gets unmarked, and a resume point never goes back.",
            color = Muted,
            style = MaterialTheme.typography.bodyMedium,
        )
        if (accounts.size < 2) {
            Text("Add both servers first (Settings → Add a server).", color = Muted)
            return@Column
        }
        Group(null) {
            ChoiceRow("From", options, from, {
                from = it
                reset()
            }, icon = Icons.Download)
            ChoiceRow("To", options, to, {
                to = it
                reset()
            }, icon = Icons.Check)
        }
        if (from == to) Text("Pick two different servers.", color = Danger)
        val b = busy
        when {
            b != null -> Text(b, color = Muted)
            plan == null && result == null ->
                ActionButton("Check what will change", { check() }, Modifier.initialFocus(), primary = true, icon = Icons.Search)
        }
        plan?.let { p ->
            val watched = p.changes.count { it.played }
            val started = p.changes.size - watched
            Text(
                if (p.changes.isEmpty()) {
                    "Nothing to copy: everything is already there."
                } else {
                    listOfNotNull(
                        "$watched to mark as watched".takeIf { watched > 0 },
                        "$started resume points".takeIf { started > 0 },
                    ).joinToString(" · ")
                },
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                listOfNotNull(
                    "${p.upToDate} already up to date".takeIf { p.upToDate > 0u },
                    "${p.notFound.size} not on that server".takeIf { p.notFound.isNotEmpty() },
                ).joinToString(" · "),
                color = Muted,
            )
            if (p.changes.isNotEmpty() && busy == null) {
                ActionButton("Copy ${p.changes.size} titles", { copy() }, Modifier.initialFocus(), primary = true, icon = Icons.Check)
            }
            if (p.notFound.isNotEmpty()) {
                ActionButton(if (showMissing) "Hide missing titles" else "Show missing titles", { showMissing = !showMissing })
                if (showMissing) {
                    Text(p.notFound.joinToString("\n"), color = Muted, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        result?.let { r ->
            Text(
                "Done: ${r.applied} titles copied" + if (r.failed > 0u) ", ${r.failed} failed (try again)." else ".",
                style = MaterialTheme.typography.titleMedium,
            )
            ActionButton("Check again", { check() })
        }
        error?.let { Text(it, color = Danger) }
    }
}
