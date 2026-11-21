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
import androidx.compose.ui.focus.focusRequester
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
import be.alwyapps.watch.ui.ConfirmDialog
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

/**
 * Seerr through the AlwyWatch server, trying each of the user's accounts it serves (Plex
 * first: Seerr usually runs in Plex mode, where a Plex user already has a Seerr account, while
 * a Jellyfin one would need creating). The first that works is kept.
 */
suspend fun connectSeerrThroughKeep(
    core: AlwyWatch,
    ctx: Context,
): be.alwyapps.watch.core.SeerrConnection {
    val keep = core.keepConnection() ?: throw CoreException.NotFound("no AlwyWatch server")
    val served = runCatching { core.keepServers(keep.baseUrl).map { it.id }.toSet() }.getOrNull()
    val candidates =
        core
            .accounts()
            .filter { served == null || it.serverId in served }
            .sortedBy { if (it.kind == ServerKind.PLEX) 0 else 1 }
    var last: CoreException? = null
    for (a in candidates) {
        val token = keepToken(ctx, a) ?: continue
        try {
            return core.seerrConnectBridgeAs(a.id, token)
        } catch (e: CoreException) {
            last = e
        }
    }
    throw last ?: CoreException.NotFound("no account to use")
}

/** Connects (or switches) the AlwyWatch server as [account], then Seerr through it if unset. */
private suspend fun connectKeep(
    core: AlwyWatch,
    ctx: Context,
    url: String,
    account: Account,
): KeepConnection {
    val token = keepToken(ctx, account) ?: throw CoreException.Invalid("Sign in to ${account.serverName} again first.")
    val conn = core.keepConnect(url, account.id, token)
    if (core.seerrConnection() == null) runCatching { connectSeerrThroughKeep(core, ctx) }
    // A new account there: the welcome step (name and picture) if it was never done.
    MyAccount.linked(core)
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
    // TV: when the group changes (connected, a list to choose from, disconnected), the focused
    // row is gone; focus goes to the first row of what replaced it, not to the next group.
    val tv = be.alwyapps.watch.ui.LocalDimens.current.isTv
    val firstFocus =
        remember {
            androidx.compose.ui.focus
                .FocusRequester()
        }
    val shape = listOf(keep?.baseUrl, keep?.accountId, served != null, choosing)
    var lastShape by remember { mutableStateOf(shape) }
    LaunchedEffect(shape) {
        if (tv && shape != lastShape) {
            androidx.compose.runtime.withFrameNanos { }
            runCatching { firstFocus.requestFocus() }
        }
        lastShape = shape
    }
    val first = if (tv) Modifier.focusRequester(firstFocus) else Modifier

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
        usable.forEachIndexed { n, a ->
            Divider()
            ListRow(
                a.label(),
                modifier = if (n == 0) first else Modifier,
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
                        Modifier.fillMaxWidth().dpadEscape().then(first),
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
            // Saved is not reachable: say so (an old address, the server down, a revoked account).
            var problem by remember(keep.baseUrl, keep.accountId) { mutableStateOf<String?>(null) }
            LaunchedEffect(keep.baseUrl, keep.accountId) {
                problem =
                    try {
                        core.keepMe()
                        null
                    } catch (e: CoreException.Unauthorized) {
                        "Your account isn't accepted there any more: choose it again below"
                    } catch (e: CoreException) {
                        "Can't reach ${keep.baseUrl}"
                    }
            }
            ListRow(
                "Syncing as ${keep.userName}" + if (keep.isAdmin) " · admin" else "",
                modifier = if (choosing) Modifier else first,
                subtitle = problem ?: listOfNotNull(syncing?.serverName, keep.baseUrl).joinToString(" · "),
                icon = Icons.Hourglass,
                iconTint = if (problem != null) Danger else Accent,
                onClick = { nav.push(Route.LeavingSoon) },
                trailing = { Chevron() },
            )
            if (problem != null) {
                Divider()
                ListRow("Use another address", subtitle = "Your keep choices stay on the server", icon = Icons.Refresh, onClick = {
                    core.keepDisconnect()
                    MyAccount.me = null
                    onChanged()
                }, trailing = { Chevron() })
            }
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
            var adding by remember { mutableStateOf(false) }
            if (adding) SendToDeviceDialog { adding = false }
            ListRow(
                "Add a device",
                subtitle = "Copy your accounts and settings to a new phone, tablet or TV",
                icon = Icons.Add,
                onClick = { adding = true },
                trailing = { Chevron() },
            )
            Divider()
            var stopping by remember { mutableStateOf(false) }
            if (stopping) {
                ConfirmDialog(
                    "Stop syncing?",
                    "Leaving soon, keep choices and watch together disappear from this device until you connect again. " +
                        "Your choices stay on the server.",
                    confirm = "Stop syncing",
                    onConfirm = {
                        core.keepDisconnect()
                        MyAccount.me = null
                        onChanged()
                    },
                    onDismiss = { stopping = false },
                )
            }
            ListRow("Stop syncing", subtitle = "Leaving soon and keep choices disappear from this device", icon = Icons.Close, iconTint = Danger, onClick = {
                stopping = true
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
                                connectSeerrThroughKeep(core, ctx)
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
