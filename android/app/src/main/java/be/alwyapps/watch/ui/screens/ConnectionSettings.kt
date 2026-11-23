// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.Account
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.KeepConnection
import be.alwyapps.watch.core.media.ServerKind
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.ConfirmDialog
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.ToggleRow
import be.alwyapps.watch.ui.tr
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

/**
 * This profile uses AlwyWatch: connected to an AlwyWatch server with a login, or signed in to
 * an AlwyWatch account. Without either it is a "Jellyfin or Plex only" profile, and everything
 * that needs the AlwyWatch server is hidden.
 */
fun hasAlwyWatch(core: AlwyWatch): Boolean = runCatching { core.keepConnection() != null || core.account() != null }.getOrDefault(false)

/**
 * Seerr through the AlwyWatch server, trying each of the user's accounts it serves (Plex
 * first: Seerr usually runs in Plex mode, where a Plex user already has a Seerr account, while
 * a Jellyfin one would need creating). The first that works is kept.
 */
suspend fun connectSeerrThroughKeep(
    core: AlwyWatch,
    ctx: Context,
): be.alwyapps.watch.core.SeerrConnection {
    val keep = core.keepConnection() ?: throw CoreException.NotFound(tr("no AlwyWatch server"))
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
    throw last ?: CoreException.NotFound(tr("no account to use"))
}

/** Connects (or switches) the AlwyWatch server as [account], then Seerr through it if unset. */
internal suspend fun connectKeep(
    core: AlwyWatch,
    ctx: Context,
    url: String,
    account: Account,
): KeepConnection {
    val token = keepToken(ctx, account) ?: throw CoreException.Invalid(tr("Sign in to {server} again first.", "server" to account.serverName))
    val conn = core.keepConnect(url, account.id, token)
    ctx.services.profiles.lastAlwyWatchUrl = conn.baseUrl
    if (core.seerrConnection() == null && core.getSetting(be.alwyapps.watch.SEERR_OFF) != "1") {
        runCatching { connectSeerrThroughKeep(core, ctx) }
    }
    // A new account there: the welcome step (name and picture) if it was never done.
    MyAccount.linked(core)
    return conn
}

/**
 * Connect to AlwyWatch with a login this profile already has (on a server the AlwyWatch server
 * serves): no new sign-in. Signed in to an AlwyWatch account, the login also joins it.
 */
internal suspend fun useExistingLogin(
    core: AlwyWatch,
    ctx: Context,
    url: String,
    account: Account,
) {
    connectKeep(core, ctx, url, account)
    keepToken(ctx, account)?.let { t -> runCatching { core.accountLinkMedia(account.id, t) } }
}

/**
 * The login used for AlwyWatch follows the login shown now: when [accountId] (just made the
 * one in use) is on the AlwyWatch server, switch to it. Nothing to do (one network call at
 * most) when it already is, or when the server doesn't serve it.
 */
suspend fun followShownLogin(
    core: AlwyWatch,
    ctx: Context,
    accountId: String,
) {
    val keep = runCatching { core.keepConnection() }.getOrNull() ?: return
    if (keep.accountId == accountId) return
    val account = runCatching { core.accounts() }.getOrNull()?.firstOrNull { it.id == accountId } ?: return
    val served = runCatching { core.keepServers(keep.baseUrl).map { it.id } }.getOrNull() ?: return
    if (account.serverId !in served) return
    runCatching { connectKeep(core, ctx, keep.baseUrl, account) }
}

/** Signs this profile out of AlwyWatch: the server connection and the account; logins stay. */
private suspend fun signOutOfAlwyWatch(
    core: AlwyWatch,
    ctx: Context,
) {
    runCatching { core.keepDisconnect() }
    MyAccount.me = null
    Identity.links = null
    if (core.account() != null) {
        runCatching { core.accountSignOut() }
        ctx.services.tokens.remove(ACCOUNT_TOKEN)
    }
}

/**
 * Settings → Account, for a profile with AlwyWatch: the AlwyWatch server (address and state; a
 * tap changes the address), password and passkeys (on the server's web page), and one "Sign
 * out of AlwyWatch". Name, picture and linked logins are drawn around it by the caller.
 */
@Composable
fun AlwyWatchConnectionGroup(
    version: Int,
    onChanged: () -> Unit,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val tv = LocalDimens.current.isTv
    val scope = rememberCoroutineScope()
    val keep = remember(version) { core.keepConnection() }
    val account = remember(version) { core.account() }
    val base = keep?.baseUrl ?: account?.baseUrl ?: return
    // Can the server be reached, and does it still accept this login?
    var problem by remember(base, keep?.accountId) { mutableStateOf<String?>(null) }
    var checked by remember(base, keep?.accountId) { mutableStateOf(false) }
    LaunchedEffect(base, keep?.accountId) {
        if (keep == null) {
            checked = true
            return@LaunchedEffect
        }
        problem =
            try {
                core.keepMe()
                null
            } catch (e: CoreException.Unauthorized) {
                tr("Sign-in expired: sign in again")
            } catch (e: CoreException) {
                tr("Can't reach it")
            }
        checked = true
    }
    var signingOut by remember { mutableStateOf(false) }
    if (signingOut) {
        ConfirmDialog(
            tr("Sign out of AlwyWatch?"),
            tr(
                "This profile keeps its Jellyfin and Plex logins and becomes a profile without an AlwyWatch account. Leaving soon and watch together disappear from it. Your keep choices stay on the server.",
            ),
            confirm = tr("Sign out"),
            onConfirm = {
                scope.launch {
                    signOutOfAlwyWatch(core, ctx)
                    onChanged()
                }
            },
            onDismiss = { signingOut = false },
        )
    }
    var passwordPage by remember { mutableStateOf<String?>(null) }
    passwordPage?.let { page -> WebPageDialog(tr("Password & passkeys"), page) { passwordPage = null } }
    val address = base.substringAfter("://").trimEnd('/')
    Group(tr("AlwyWatch server")) {
        ListRow(
            address,
            subtitle =
                when {
                    problem != null -> problem
                    !checked -> tr("Checking…")
                    keep != null && account == null -> tr("Connected with your login {name}", "name" to keep.userName)
                    else -> tr("Connected")
                },
            icon = Icons.Hourglass,
            iconTint = if (problem != null) Danger else Accent,
            onClick = { nav.push(Route.ConnectAlwyWatch(create = false)) },
            trailing = { Chevron() },
        )
        // The account itself is managed on the server's website (one place for every app).
        if (account != null) {
            Divider()
            val page = account.baseUrl.trimEnd('/') + "/account"
            ListRow(
                tr("Password & passkeys"),
                subtitle =
                    if (tv) {
                        tr("Also deleting the account: on your phone or computer, at {address}", "address" to page.substringAfter("://"))
                    } else {
                        tr("Also deleting the account: opens {address}", "address" to page.substringAfter("://"))
                    },
                icon = Icons.Settings,
                onClick = { if (tv) passwordPage = page else openInBrowser(ctx, page) },
                trailing = { Chevron() },
            )
        }
    }
    MyAccount.me?.let { LinkedSignInsGroup(it) }
    Group(null) {
        ListRow(
            tr("Sign out of AlwyWatch"),
            subtitle = tr("This profile keeps its Jellyfin and Plex logins"),
            icon = Icons.Close,
            iconTint = Danger,
            onClick = { signingOut = true },
        )
    }
}

/** TV: a web page to open on another device, as a QR code and its address. */
@Composable
internal fun WebPageDialog(
    title: String,
    page: String,
    onDismiss: () -> Unit,
) {
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Background,
        title = { Text(title) },
        text = {
            Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
                QrImage(page)
                Text(tr("Scan with your phone, or open"), color = Muted, style = MaterialTheme.typography.bodyMedium)
                Text(page.substringAfter("://"), style = MaterialTheme.typography.titleMedium)
            }
        },
        confirmButton = { ActionButton(tr("Close"), onDismiss) },
    )
}

/**
 * Settings → Account, for a profile without AlwyWatch: the profile's own name on this device,
 * and a way to connect it (the same screens as "Add a profile").
 */
@Composable
fun LocalProfileGroups() {
    val ctx = LocalContext.current
    val nav = LocalNavigator.current
    val core = LocalCore.current
    val services = ctx.services
    var profile by remember { mutableStateOf(currentProfile(services, core)) }
    var renaming by remember { mutableStateOf(false) }
    val p = profile
    if (p != null) {
        if (renaming) {
            var name by remember { mutableStateOf(p.name) }
            androidx.compose.material3.AlertDialog(
                onDismissRequest = { renaming = false },
                containerColor = Background,
                title = { Text(tr("Your name")) },
                text = {
                    Field(name, { name = it.take(24) }, Modifier.fillMaxWidth(), placeholder = { Text(tr("Name")) }, singleLine = true)
                },
                confirmButton = {
                    ActionButton(tr("Save"), {
                        services.profiles.rename(p.id, name)
                        profile = currentProfile(services, core)
                        renaming = false
                    }, primary = true)
                },
                dismissButton = { ActionButton(tr("Cancel"), { renaming = false }) },
            )
        }
        Group(tr("This profile"), footer = tr("Without an AlwyWatch account, the name stays on this device.")) {
            ListRow(p.name, subtitle = tr("Profile on this device"), leading = { Avatar(p.name, 46) })
            Divider()
            ListRow(tr("Change name"), icon = Icons.Person, onClick = { renaming = true }, trailing = { Chevron() })
        }
    }
    Group(
        tr("Connect to AlwyWatch"),
        footer = tr("Your name, settings and servers on every device, plus Leaving soon and watch together. You need an address from your server's owner."),
    ) {
        ListRow(
            tr("Sign in"),
            subtitle = tr("With your AlwyWatch account, a code, or your Jellyfin or Plex login"),
            icon = Icons.Person,
            onClick = { nav.push(Route.ConnectAlwyWatch(create = false)) },
            trailing = { Chevron() },
        )
        Divider()
        ListRow(
            tr("Create an account"),
            subtitle = tr("You have an invitation"),
            icon = Icons.Add,
            onClick = { nav.push(Route.ConnectAlwyWatch(create = true)) },
            trailing = { Chevron() },
        )
    }
}

/**
 * Settings → Requests: through the AlwyWatch server (a switch when it takes requests), or a
 * request site (Seerr) directly by its address.
 */
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
    val services =
        be.alwyapps.watch.ui
            .keepServices()
    var error by remember(version) { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }

    fun turnOff() {
        core.seerrSignOut()
        core.setSetting(be.alwyapps.watch.SEERR_OFF, "1")
        ctx.services.tokens.remove("seerr")
        onChanged()
    }

    if (keep != null) {
        Group(tr("Through your AlwyWatch server")) {
            when {
                seerr?.viaKeep == true ->
                    ToggleRow(
                        tr("Requests"),
                        true,
                        { turnOff() },
                        subtitle = tr("Through your AlwyWatch server, as {name}", "name" to seerr.userName),
                    )
                services.requests ->
                    ToggleRow(
                        tr("Requests"),
                        false,
                        {
                            if (!busy) {
                                busy = true
                                error = null
                                scope.launch {
                                    try {
                                        core.setSetting(be.alwyapps.watch.SEERR_OFF, "0")
                                        connectSeerrThroughKeep(core, ctx)
                                        onChanged()
                                    } catch (e: CoreException) {
                                        error = e.message
                                    } finally {
                                        busy = false
                                    }
                                }
                            }
                        },
                        subtitle =
                            if (seerr != null) {
                                tr("Off: you use a request site directly")
                            } else if (busy) {
                                tr("Connecting…")
                            } else {
                                tr("Off")
                            },
                    )
                else ->
                    Text(
                        tr("Your AlwyWatch server doesn't take requests. Ask its owner, or use a request site address below."),
                        Modifier.padding(16.dp),
                        color = Muted,
                        style = MaterialTheme.typography.bodyMedium,
                    )
            }
            error?.let { Text(it, Modifier.padding(16.dp), color = Danger, style = MaterialTheme.typography.bodySmall) }
        }
    }
    Group(
        tr("Request site"),
        footer =
            tr(
                "Ask for movies and shows that aren't on the server yet, on a request site (Seerr, Overseerr or Jellyseerr) run by your server's owner. It only accepts the logins it's set up for (Plex or Jellyfin).",
            ),
    ) {
        if (seerr != null && !seerr.viaKeep) {
            ListRow(
                seerr.baseUrl.substringAfter("://").trimEnd('/'),
                subtitle = tr("Signed in as {name}", "name" to seerr.userName),
                icon = Icons.Add,
                iconTint = Accent,
            )
            Divider()
            ListRow(tr("Sign out"), icon = Icons.Close, iconTint = Danger, onClick = { turnOff() })
        } else {
            ListRow(
                tr("Use a request site address"),
                subtitle = tr("It only accepts the logins it's set up for (Plex or Jellyfin)"),
                icon = Icons.Add,
                onClick = { nav.push(Route.SeerrConnect) },
                trailing = { Chevron() },
            )
        }
    }
}
