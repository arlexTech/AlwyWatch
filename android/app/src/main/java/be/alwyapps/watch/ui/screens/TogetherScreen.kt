// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.player.PlayerActivity
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.initialFocus
import kotlinx.coroutines.launch

/**
 * Watch together: type the code a friend sees in their player. The room's title is looked up on
 * your own servers (each plays their own copy), then the player opens in sync with the room.
 */
@Composable
fun TogetherScreen() {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var code by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun join() {
        if (busy || code.length != 6) return
        busy = true
        error = null
        scope.launch {
            try {
                val joined = core.togetherJoin(code)
                val receivedAt = android.os.SystemClock.elapsedRealtime()
                val accounts = runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id }
                val found = core.togetherFind(preferredFirst(core, accounts), joined.state.media)
                if (found == null) {
                    runCatching { core.togetherLeave(joined.state.code, joined.memberId) }
                    val m = joined.state.media
                    val what = m.seriesTitle?.let { "$it S${m.season} · E${m.episode}" } ?: m.title
                    error = "$what isn't on your servers."
                    return@launch
                }
                val start =
                    be.alwyapps.watch.core
                        .togetherPosition(joined.state, receivedAt, android.os.SystemClock.elapsedRealtime())
                        .toLong()
                ctx.startActivity(
                    PlayerActivity.intent(ctx, found.accountId, found.itemId, start, joined.state.code, joined.memberId),
                )
                code = ""
            } catch (e: CoreException) {
                error = if (e is CoreException.NotFound) "No room with this code (it may have closed)." else e.message
            } finally {
                busy = false
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 24.dp)
            .widthIn(max = 560.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        PageTitle("Watch together")
        Text(
            "Play the same movie or episode in sync with friends, each on their own server. " +
                "To start a room, press “Watch together” in the player and share its code.",
            color = Muted,
            style = MaterialTheme.typography.bodyMedium,
        )
        Field(
            code,
            { code = it.filter(Char::isDigit).take(6) },
            Modifier.initialFocus(),
            placeholder = { Text("Room code (6 digits)") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number, imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { join() }),
            leadingIcon = Icons.Group,
        )
        ActionButton(if (busy) "Joining…" else "Join", { join() }, primary = true, icon = Icons.Group)
        error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodyMedium) }
    }
}
