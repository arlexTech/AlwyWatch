// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.seasonPlan
import be.alwyapps.watch.core.seasonsRequestedMessage
import be.alwyapps.watch.core.seerr.SeasonState
import be.alwyapps.watch.core.seerr.SeerrMediaType
import be.alwyapps.watch.core.seerr.SeerrSeasonInfo
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.focusOnAppear
import be.alwyapps.watch.ui.ignoreHeldOk
import kotlinx.coroutines.launch

/** A Seerr request error in plain words (the web app's wording). */
fun requestErrorText(e: CoreException): String =
    when (e) {
        // Seerr answers 403 both when the quota is used up and when the person may not request.
        is CoreException.Unauthorized ->
            "Seerr refused: you may have no requests left, or not be allowed to request. If it keeps happening, sign in again in Settings."
        is CoreException.Network -> "Can't reach Seerr right now."
        is CoreException.NotFound -> "Seerr doesn't know this title."
        is CoreException.Server ->
            if (e.message.orEmpty().contains("409")) "It has already been requested." else "Seerr had a problem. Try again in a moment."
        else -> e.message ?: "Something went wrong."
    }

/**
 * A season's row label in the app's language: "Season N" from its number when the server's name
 * is only a word and that number ("Saison 2", "Staffel 2"); specials and named seasons keep
 * their name.
 */
internal fun seasonLabel(
    number: UInt,
    name: String,
): String {
    if (number == 0u) return name.ifBlank { "Specials" }
    val generic = name.isBlank() || Regex("^\\p{L}+\\.?\\s*0*$number$").matches(name.trim())
    return if (generic) "Season $number" else name
}

/** A square check box (the row around it is the click and focus target). */
@Composable
private fun CheckBox(checked: Boolean) {
    Box(
        Modifier
            .size(24.dp)
            .background(if (checked) Accent else Color.Transparent, RoundedCornerShape(6.dp))
            .border(2.dp, if (checked) Accent else Muted, RoundedCornerShape(6.dp)),
        contentAlignment = Alignment.Center,
    ) {
        if (checked) Icon(Icons.Check, null, Modifier.size(18.dp), tint = Color.Black)
    }
}

/** The mark of a season that can't be picked (on the server, requested…). */
@Composable
private fun StateMark(state: SeasonState) {
    Box(Modifier.size(24.dp), contentAlignment = Alignment.Center) {
        Icon(
            when (state) {
                SeasonState.ON_SERVER, SeasonState.PARTLY_ON_SERVER -> Icons.Check
                SeasonState.DOWNLOADING -> Icons.Download
                SeasonState.REQUESTED -> Icons.Hourglass
                else -> Icons.Close
            },
            null,
            Modifier.size(20.dp),
            tint = if (state == SeasonState.ON_SERVER) Accent else Subtle,
        )
    }
}

/**
 * "Request more seasons" for a show of the library: its seasons as Seerr knows them, each with
 * its state; the ones that can be requested are check boxes. [have]: the season numbers the
 * library shows. [onDismiss] gets the latest seasons (they change once something is requested).
 */
@Composable
fun RequestSeasonsDialog(
    tmdbId: Long,
    showTitle: String,
    have: List<UInt>,
    initial: List<SeerrSeasonInfo>,
    onDismiss: (List<SeerrSeasonInfo>) -> Unit,
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var info by remember { mutableStateOf(initial) }
    var chosen by remember { mutableStateOf(setOf<UInt>()) }
    var busy by remember { mutableStateOf(false) }
    var done by remember { mutableStateOf<String?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    val plan = remember(info, have) { seasonPlan(info, have) }
    val close = { onDismiss(info) }

    fun send() {
        if (busy || chosen.isEmpty()) return
        busy = true
        error = null
        val numbers = chosen.sorted()
        scope.launch {
            try {
                core.seerrRequest(SeerrMediaType.TV, tmdbId, numbers)
                done = seasonsRequestedMessage(numbers)
                chosen = emptySet()
                // The new states (requested, maybe already downloading).
                runCatching { core.seerrSeasonInfo(tmdbId) }.onSuccess { info = it }
            } catch (e: CoreException) {
                error = "Couldn't request them: ${requestErrorText(e)}"
            }
            busy = false
        }
    }
    val firstOpen = plan.missing.firstOrNull()
    AlertDialog(
        onDismissRequest = close,
        containerColor = Background,
        title = {
            Column {
                Text("Request more seasons")
                Text(showTitle, color = Muted, style = androidx.compose.material3.MaterialTheme.typography.bodyMedium)
            }
        },
        text = {
            Column(Modifier.ignoreHeldOk()) {
                Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
                    for (s in plan.seasons) {
                        val open = s.state == SeasonState.REQUESTABLE
                        val on = s.number in chosen
                        if (open) {
                            ListRow(
                                seasonLabel(s.number, s.name),
                                if (s.number == firstOpen) Modifier.focusOnAppear() else Modifier,
                                subtitle = s.label,
                                leading = { CheckBox(on) },
                                onClick = { chosen = if (on) chosen - s.number else chosen + s.number },
                            )
                        } else {
                            ListRow(seasonLabel(s.number, s.name), subtitle = s.label, leading = { StateMark(s.state) })
                        }
                    }
                }
                done?.let { Text(it, Modifier.padding(top = 12.dp), color = Accent) }
                error?.let { Text(it, Modifier.padding(top = 12.dp), color = Danger) }
            }
        },
        dismissButton = {
            if (plan.missing.size > 1) {
                ActionButton("Select all missing", { chosen = plan.missing.toSet() }, Modifier.ignoreHeldOk())
            }
        },
        confirmButton = {
            if (plan.missing.isEmpty()) {
                // Everything is there or asked for (after a request): the way out, focused.
                ActionButton("Close", close, Modifier.ignoreHeldOk().focusOnAppear(), primary = true)
            } else {
                val n = chosen.size
                ActionButton(
                    when {
                        busy -> "Requesting…"
                        n == 1 -> "Request 1 season"
                        else -> "Request $n seasons"
                    },
                    { send() },
                    // Dimmed at 0, but still focusable: it keeps the focus once a request is sent.
                    Modifier.ignoreHeldOk().then(if (n == 0) Modifier.alpha(0.4f) else Modifier),
                    primary = true,
                    icon = Icons.Add,
                )
            }
        },
    )
}
