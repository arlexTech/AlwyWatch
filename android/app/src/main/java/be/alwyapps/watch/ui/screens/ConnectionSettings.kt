// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import be.alwyapps.watch.core.Account
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.KeepConnection
import be.alwyapps.watch.core.keep.ServerEntry
import be.alwyapps.watch.core.media.ServerKind
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.OnSurface
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.Value
import be.alwyapps.watch.ui.dpadEscape
import kotlinx.coroutines.launch

/** The token the AlwyWatch server accepts for an account: Jellyfin's, or Plex's plex.tv one. */
fun keepToken(
    ctx: Context,
    account: Account,
): String? =
    if (account.kind == ServerKind.PLEX) {
        ctx.services.tokens.get("${account.id}:plextv")
    } else {
        ctx.services.tokens.get(account.id)
    }

private fun Account.label() = "$userName · $serverName (${if (kind == ServerKind.PLEX) "Plex" else "Jellyfin"})"

/** Connects (or switches) the AlwyWatch server as [account], then Seerr through it if unset. */
private suspend fun connectKeep(
    core: AlwyWatch,
    ctx: Context,
    url: String,
    account: Account,
): KeepConnection {
    val token = keepToken(ctx, account) ?: throw CoreException.Invalid("Sign in to ${account.serverName} again first.")
    val conn = core.keepConnect(url, account.id, token)
    if (core.seerrConnection() == null) runCatching { core.seerrConnectBridge() }
    return conn
}

/**
 * Settings group for the AlwyWatch server: connect at any time (also after signing in
 * directly), choose which of your accounts syncs with it, switch, or stop syncing.
 */
@Composable
fun AlwyWatchServerGroup(
    accounts: List<Account>,
    version: Int,
    onChanged: () -> Unit,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val keep = remember(version) { core.keepConnection() }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    // Address being connected (not connected yet) and the servers it serves.
    var url by remember { mutableStateOf("") }
    var served by remember(version) { mutableStateOf<List<ServerEntry>?>(null) }
    var choosing by remember(version) { mutableStateOf(false) }

    fun attempt(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try {
                block()
            } catch (e: CoreException) {
                error = e.message
            } finally {
                busy = false
            }
        }
    }

    // Which accounts the server can sync: the ones on a media server it serves.
    @Composable
    fun AccountChoices(
        base: String,
        list: List<ServerEntry>,
        current: String?,
    ) {
        val ids = list.map { it.id }.toSet()
        val usable = accounts.filter { it.serverId in ids }
        Text(
            if (usable.isEmpty()) {
                "None of your accounts is on a server it serves (${list.joinToString { it.name }}). Add one of those first."
            } else {
                "Choose the account that syncs with it."
            },
            Modifier.padding(horizontal = 16.dp, vertical = 12.dp),
            style = MaterialTheme.typography.bodySmall,
            color = Muted,
        )
        usable.forEach { a ->
            Divider()
            ListRow(
                a.label(),
                icon = Icons.Person,
                iconTint = if (a.id == current) Accent else OnSurface,
                onClick = if (a.id == current) null else ({ attempt { connectKeep(core, ctx, base, a).also { onChanged() } } }),
                trailing = { if (a.id == current) Value("Syncing") else Chevron() },
            )
        }
        if (usable.isEmpty()) {
            Divider()
            ListRow("Add a server", icon = Icons.Add, onClick = { nav.push(Route.AddServer) }, trailing = { Chevron() })
        }
    }

    Group(
        "AlwyWatch server",
        footer = "Shows what your server will delete and lets you keep it. Your Jellyfin or Plex account is your sign-in there.",
    ) {
        if (keep == null) {
            val list = served
            if (list == null) {
                val find = {
                    attempt {
                        val base =
                            be.alwyapps.watch.core
                                .normalizeServerUrl(url)
                        val l = core.keepServers(base)
                        url = base
                        // One matching account: nothing to choose.
                        val match = accounts.filter { a -> l.any { it.id == a.serverId } }
                        if (match.size == 1) {
                            connectKeep(core, ctx, base, match.single())
                            onChanged()
                        } else {
                            served = l
                        }
                    }
                }
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Text("Not connected. Got an address from your server's owner?", style = MaterialTheme.typography.bodyMedium, color = Muted)
                    Field(
                        url,
                        { url = it },
                        Modifier.fillMaxWidth().dpadEscape(),
                        placeholder = { Text("watch.example.org") },
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                        keyboardActions = KeyboardActions(onGo = { find() }),
                    )
                    ActionButton(if (busy) "Connecting…" else "Connect", { find() }, primary = true)
                }
            } else {
                AccountChoices(url, list, null)
            }
        } else {
            val syncing = accounts.firstOrNull { it.id == keep.accountId }
            ListRow(
                "Syncing as ${keep.userName}" + if (keep.isAdmin) " · admin" else "",
                subtitle = listOfNotNull(syncing?.serverName, keep.baseUrl).joinToString(" · "),
                icon = Icons.Hourglass,
                iconTint = Accent,
                onClick = { nav.push(Route.LeavingSoon) },
                trailing = { Chevron() },
            )
            if (keep.isAdmin) {
                Divider()
                ListRow("Admin", subtitle = "Rights and countdown", icon = Icons.Settings, onClick = { nav.push(Route.KeepAdmin) }, trailing = {
                    Chevron()
                })
            }
            if (choosing) {
                val list = served
                if (list == null) {
                    LaunchedEffect(keep.baseUrl) {
                        try {
                            served = core.keepServers(keep.baseUrl)
                        } catch (e: CoreException) {
                            error = e.message
                        }
                    }
                } else {
                    AccountChoices(keep.baseUrl, list, keep.accountId)
                }
            } else if (accounts.size > 1) {
                Divider()
                ListRow("Sync another account", subtitle = "Pick which of your servers it follows", icon = Icons.Person, onClick = {
                    choosing = true
                }, trailing = { Chevron() })
            }
            Divider()
            ListRow("Stop syncing", subtitle = "Leaving soon and keep choices disappear from this device", icon = Icons.Close, iconTint = Danger, onClick = {
                core.keepDisconnect()
                onChanged()
            })
        }
        error?.let { Text(it, Modifier.padding(16.dp), color = Danger, style = MaterialTheme.typography.bodySmall) }
    }
}

/** Settings group for Seerr: through the AlwyWatch server, or directly; and stop. */
@Composable
fun RequestsGroup(
    version: Int,
    onChanged: () -> Unit,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val seerr = remember(version) { core.seerrConnection() }
    val keep = remember(version) { core.keepConnection() }
    var error by remember(version) { mutableStateOf<String?>(null) }

    Group("Requests", footer = "Seerr lets you ask for movies and shows that aren't on the server yet.") {
        if (seerr == null) {
            if (keep != null) {
                ListRow(
                    "Use your AlwyWatch server",
                    subtitle = "Nothing to sign in to: it's your ${keep.userName} account",
                    icon = Icons.Hourglass,
                    onClick = {
                        scope.launch {
                            try {
                                core.seerrConnectBridge()
                                onChanged()
                            } catch (e: CoreException) {
                                error = "Your AlwyWatch server doesn't offer requests (${e.message})."
                            }
                        }
                    },
                    trailing = { Chevron() },
                )
                Divider()
            }
            ListRow(
                if (keep != null) "Connect Seerr directly" else "Connect Seerr",
                subtitle = "With its address and your account there",
                icon = Icons.Add,
                onClick = { nav.push(Route.SeerrConnect) },
                trailing = { Chevron() },
            )
        } else {
            ListRow(
                "Requests",
                subtitle = "Yours and everyone's · as ${seerr.userName}" + if (seerr.viaKeep) ", through your AlwyWatch server" else "",
                icon = Icons.Add,
                iconTint = Accent,
                onClick = { nav.push(Route.Requests) },
                trailing = { Chevron() },
            )
            Divider()
            ListRow("Stop using Seerr", icon = Icons.Close, iconTint = Danger, onClick = {
                core.seerrSignOut()
                ctx.services.tokens.remove("seerr")
                onChanged()
            })
        }
        error?.let { Text(it, Modifier.padding(16.dp), color = Danger, style = MaterialTheme.typography.bodySmall) }
    }
}
