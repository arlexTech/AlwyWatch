// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
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
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn

/**
 * Copy watch history from one server to another (Plex → Jellyfin when moving): watched titles
 * and resume points. Checks first and shows what will change; writes only on "Copy". Only adds:
 * nothing is unmarked, no resume point goes backwards.
 */
@Composable
fun HistoryCopyScreen() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val ctx = LocalContext.current
    val accounts = remember { runCatching { core.accounts() }.getOrDefault(emptyList()) }
    val options =
        accounts.map {
            it.id to "${it.serverName} · ${it.userName} (${if (it.kind == ServerKind.PLEX) "Plex" else "Jellyfin"})"
        }
    // Plex to Jellyfin by default: the usual move.
    val job = be.alwyapps.watch.history.HistoryJob
    // Plex to Jellyfin by default: the usual move.
    val from = job.from ?: (accounts.firstOrNull { it.kind == ServerKind.PLEX }?.id ?: accounts.firstOrNull()?.id.orEmpty())
    val to = job.to ?: (accounts.firstOrNull { it.kind == ServerKind.JELLYFIN && it.id != from }?.id ?: accounts.firstOrNull { it.id != from }?.id.orEmpty())
    val plan = job.plan
    val result = job.result
    val error = job.error
    var showMissing by remember { mutableStateOf(false) }
    // While it runs: where it is, twice a second.
    var progress by remember { mutableStateOf<be.alwyapps.watch.core.HistoryProgress?>(null) }
    androidx.compose.runtime.LaunchedEffect(job.running) {
        while (job.running) {
            progress = runCatching { core.historyCopyProgress() }.getOrNull()
            kotlinx.coroutines.delay(500)
        }
        progress = null
    }

    // The progress lives in a notification: ask once (Android 13+), then check either way.
    val notify =
        androidx.activity.compose.rememberLauncherForActivityResult(
            androidx.activity.result.contract.ActivityResultContracts
                .RequestPermission(),
        ) { }

    fun check() {
        if (from == to) return
        if (android.os.Build.VERSION.SDK_INT >= 33 &&
            ctx.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            notify.launch(android.Manifest.permission.POST_NOTIFICATIONS)
        }
        job.from = from
        job.to = to
        job.check(ctx, from, to)
    }

    fun copy() = job.copy(ctx)

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 24.dp)
            .widthIn(max = 640.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        PageTitle(tr("Copy watch history"))
        Text(
            tr(
                "Marks what you watched on one server as watched on the other, and copies where you stopped. It only adds: nothing gets unmarked, and a resume point never goes back.",
            ),
            color = Muted,
            style = MaterialTheme.typography.bodyMedium,
        )
        if (accounts.size < 2) {
            Text(tr("Sign in to both servers first: Settings → Servers → Add a server."), color = Muted)
            return@Column
        }
        Group(null) {
            ChoiceRow(tr("From"), options, from, {
                job.from = it
                job.reset()
            }, icon = Icons.Download, mark = {
                be.alwyapps.watch.ui
                    .ServerBadge(it, diameter = 28.dp)
            })
            ChoiceRow(tr("To"), options, to, {
                job.to = it
                job.reset()
            }, icon = Icons.Check, mark = {
                be.alwyapps.watch.ui
                    .ServerBadge(it, diameter = 28.dp)
            })
        }
        if (from == to) Text(tr("Pick two different servers."), color = Danger)
        when {
            job.running -> RunningProgress(progress)
            plan == null && result == null ->
                ActionButton(tr("Check what will change"), { check() }, Modifier.initialFocus(), primary = true, icon = Icons.Search)
        }
        plan?.let { p ->
            val watched = p.changes.count { it.played }
            val started = p.changes.size - watched
            Text(
                if (p.changes.isEmpty()) {
                    tr("Nothing to copy: everything is already there.")
                } else {
                    listOfNotNull(
                        tr("{count} to mark as watched", "count" to watched).takeIf { watched > 0 },
                        trn("{n} resume point", "{n} resume points", started).takeIf { started > 0 },
                    ).joinToString(" · ")
                },
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                listOfNotNull(
                    tr("{count} already up to date", "count" to p.upToDate).takeIf { p.upToDate > 0u },
                    tr("{count} not on that server", "count" to p.notFound.size).takeIf { p.notFound.isNotEmpty() },
                ).joinToString(" · "),
                color = Muted,
            )
            if (p.changes.isNotEmpty() && !job.running) {
                ActionButton(trn("Copy {n} title", "Copy {n} titles", p.changes.size), { copy() }, Modifier.initialFocus(), primary = true, icon = Icons.Check)
            }
            if (p.notFound.isNotEmpty()) {
                ActionButton(if (showMissing) tr("Hide missing titles") else tr("Show missing titles"), { showMissing = !showMissing })
                if (showMissing) {
                    Text(p.notFound.joinToString("\n"), color = Muted, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        result?.let { r ->
            Text(
                if (r.failed > 0u) {
                    tr("Done: {copied} titles copied, {failed} failed (try again).", "copied" to r.applied, "failed" to r.failed)
                } else {
                    trn("Done: {n} title copied.", "Done: {n} titles copied.", r.applied.toLong())
                },
                style = MaterialTheme.typography.titleMedium,
            )
            ActionButton(tr("Check again"), { check() })
        }
        error?.let { Text(it, color = Danger) }
    }
}

/** The step it is on, a bar (exact when the count is known), and that leaving is fine. */
@Composable
private fun RunningProgress(p: be.alwyapps.watch.core.HistoryProgress?) {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        val total = p?.total?.toInt() ?: 0
        val done = p?.done?.toInt() ?: 0
        Text(
            (p?.step?.ifEmpty { null } ?: tr("Starting")).let { step ->
                if (total > 0) {
                    tr("{step} · {done} of {total} ({percent} %)", "step" to step, "done" to done, "total" to total, "percent" to done * 100 / total)
                } else {
                    tr("{step}…", "step" to step)
                }
            },
            style = MaterialTheme.typography.titleSmall,
        )
        if (total > 0) {
            androidx.compose.material3.LinearProgressIndicator(
                progress = { done.toFloat() / total },
                modifier = Modifier.fillMaxWidth(),
                color = be.alwyapps.watch.ui.Accent,
            )
        } else {
            androidx.compose.material3.LinearProgressIndicator(Modifier.fillMaxWidth(), color = be.alwyapps.watch.ui.Accent)
        }
        Text(
            tr("You can leave the app: it keeps going (see the notification), and this page shows where it is when you come back."),
            color = Muted,
            style = MaterialTheme.typography.bodySmall,
        )
    }
}
