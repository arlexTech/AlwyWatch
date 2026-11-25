// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.SliderDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.RemoteAction
import be.alwyapps.watch.core.keep.RemoteState
import be.alwyapps.watch.core.remotePositionNow
import be.alwyapps.watch.player.formatTime
import be.alwyapps.watch.remote.RemotePhone
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.ChoiceDialog
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.Value
import be.alwyapps.watch.ui.tr
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * The mini remote of a TV (Play on the TV): what it plays, where it is, pause, jump, tracks
 * and stop. Follows the TV's reports (a long poll); with no [deviceId], the list of my TVs.
 */
@Composable
fun RemoteScreen(deviceId: String?) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    val devices = RemotePhone.devices
    LaunchedEffect(Unit) { RemotePhone.refresh(core) }
    // One TV: straight to it.
    val id = deviceId ?: devices.singleOrNull()?.id
    if (id == null) {
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = d.gutter, vertical = 16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            PageTitle(tr("Play on the TV"))
            if (devices.isEmpty()) {
                Text(tr("No TV here. Open AlwyWatch on the TV, with this profile, and it shows here."), color = Muted)
            } else {
                Group(null) {
                    devices.forEach { tv ->
                        ListRow(
                            tv.name,
                            subtitle = tv.title.ifEmpty { tr("Nothing playing") },
                            icon = Icons.Tv,
                            onClick = { nav.push(Route.Remote(tv.id)) },
                        )
                    }
                }
            }
        }
        return
    }
    val name = devices.firstOrNull { it.id == id }?.name ?: tr("TV")
    var state by remember(id) { mutableStateOf<RemoteState?>(null) }
    var gone by remember(id) { mutableStateOf(false) }
    // The position shown: the TV's, moving on while it plays (or where the slider is dragged).
    var now by remember(id) { mutableLongStateOf(0L) }
    var dragging by remember { mutableStateOf<Float?>(null) }
    var picking by remember { mutableStateOf<RemoteAction?>(null) }
    LaunchedEffect(id) {
        var after = 0uL
        var failures = 0
        while (true) {
            try {
                val s = core.remoteState(id, after)
                state = s
                after = s.version
                gone = false
                failures = 0
            } catch (_: CoreException.NotFound) {
                gone = true
                delay(5_000)
            } catch (_: CoreException) {
                failures++
                delay((failures * 2_000L).coerceAtMost(15_000))
            }
        }
    }
    LaunchedEffect(state) {
        while (true) {
            state?.let { now = remotePositionNow(it) }
            delay(500)
        }
    }

    fun send(
        action: RemoteAction,
        position: Long? = null,
        index: Int? = null,
    ) {
        scope.launch { runCatching { core.remoteControl(id, action, position, index) } }
    }

    val r = state?.report
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        PageTitle(name, tr("Play on the TV"))
        when {
            gone -> Text(tr("This TV isn't there anymore: AlwyWatch was closed on it."), color = Muted)
            r == null -> Text(tr("Connecting…"), color = Muted)
            else -> {
                r.error?.let { Text(it, color = Accent) }
                if (r.title.isEmpty()) {
                    Text(tr("Nothing playing. Pick a title, then Play and this TV."), color = Muted)
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(r.title, style = MaterialTheme.typography.headlineSmall)
                        r.subtitle?.let { Text(it, color = Muted, style = MaterialTheme.typography.bodyMedium) }
                        if (r.loading) Text(tr("Loading…"), color = Muted, style = MaterialTheme.typography.bodySmall)
                    }
                    val duration = r.durationMs.coerceAtLeast(1)
                    Column {
                        Slider(
                            value = dragging ?: (now.toFloat() / duration).coerceIn(0f, 1f),
                            onValueChange = { dragging = it },
                            onValueChangeFinished = {
                                dragging?.let { send(RemoteAction.SEEK, (it * duration).toLong()) }
                                dragging = null
                            },
                            colors = SliderDefaults.colors(thumbColor = Accent, activeTrackColor = Accent),
                        )
                        Row(Modifier.fillMaxWidth()) {
                            Text(formatTime(dragging?.let { (it * duration).toLong() } ?: now), color = Muted, style = MaterialTheme.typography.bodySmall)
                            Spacer(Modifier.weight(1f))
                            Text(formatTime(r.durationMs), color = Muted, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterHorizontally),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        IconAction(Icons.Replay10, tr("Back 10 seconds"), { send(RemoteAction.SEEK, (now - 10_000).coerceAtLeast(0)) })
                        if (r.playing) {
                            ActionButton(tr("Pause"), { send(RemoteAction.PAUSE) }, primary = true, icon = Icons.Pause)
                        } else {
                            ActionButton(tr("Play"), { send(RemoteAction.RESUME) }, primary = true, icon = Icons.Play)
                        }
                        IconAction(Icons.Forward30, tr("Forward 30 seconds"), { send(RemoteAction.SEEK, (now + 30_000).coerceAtMost(r.durationMs)) })
                    }
                    Group(null) {
                        if (r.audio.size > 1) {
                            ListRow(
                                tr("Audio"),
                                icon = Icons.Volume,
                                onClick = { picking = RemoteAction.AUDIO },
                                trailing = {
                                    Value(
                                        r.audio
                                            .firstOrNull { it.index == r.audioIndex }
                                            ?.label
                                            .orEmpty(),
                                    )
                                },
                            )
                        }
                        ListRow(
                            tr("Subtitles"),
                            icon = Icons.Subtitles,
                            onClick = { picking = RemoteAction.SUBTITLES },
                            trailing = { Value(r.subtitles.firstOrNull { it.index == r.subtitleIndex }?.label ?: tr("Off")) },
                        )
                        ListRow(tr("Stop"), icon = Icons.Close, onClick = { send(RemoteAction.STOP) })
                    }
                }
            }
        }
        Spacer(Modifier.height(8.dp))
    }
    val report = r
    when (picking) {
        RemoteAction.AUDIO ->
            if (report != null) {
                ChoiceDialog(
                    tr("Audio"),
                    report.audio.map { it.index.toString() to it.label },
                    report.audioIndex?.toString(),
                    onPick = {
                        send(RemoteAction.AUDIO, index = it.toInt())
                        picking = null
                    },
                    onDismiss = { picking = null },
                )
            }
        RemoteAction.SUBTITLES ->
            if (report != null) {
                ChoiceDialog(
                    tr("Subtitles"),
                    listOf("" to tr("Off")) + report.subtitles.map { it.index.toString() to it.label },
                    report.subtitleIndex?.toString() ?: "",
                    onPick = {
                        send(RemoteAction.SUBTITLES, index = it.toIntOrNull())
                        picking = null
                    },
                    onDismiss = { picking = null },
                )
            }
        else -> {}
    }
}

/** "Where to play?": this phone or one of my TVs (shown while [RemotePhone.asking] is set). */
@Composable
fun RemotePlayChooser() {
    val pending = RemotePhone.asking ?: return
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var error by remember(pending) { mutableStateOf<String?>(null) }
    val here = "\u0000here"
    ChoiceDialog(
        tr("Play on"),
        listOf(here to tr("This phone")) + RemotePhone.devices.map { it.id to it.name },
        null,
        onPick = { key ->
            if (key == here) {
                RemotePhone.asking = null
                RemotePhone.here(ctx, pending.account, pending.itemId, pending.startMs)
            } else {
                scope.launch {
                    try {
                        core.remotePlay(key, pending.account, pending.itemId, pending.startMs)
                        RemotePhone.asking = null
                        nav.push(Route.Remote(key))
                    } catch (e: CoreException) {
                        error = e.message
                    }
                }
            }
        },
        onDismiss = { RemotePhone.asking = null },
        mark = { key ->
            androidx.compose.material3.Icon(
                if (key == here) Icons.Play else Icons.Tv,
                null,
                tint = Muted,
            )
        },
    )
    error?.let {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { error = null },
            text = { Text(it) },
            confirmButton = { ActionButton(tr("Close"), { error = null }) },
            containerColor = be.alwyapps.watch.ui.Background,
        )
    }
}
