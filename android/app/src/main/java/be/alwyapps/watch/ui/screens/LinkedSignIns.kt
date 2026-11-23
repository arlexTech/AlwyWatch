// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import be.alwyapps.watch.core.Account
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.LinkedIdentity
import be.alwyapps.watch.core.keep.Me
import be.alwyapps.watch.core.keep.ServerType
import be.alwyapps.watch.core.media.ServerKind
import be.alwyapps.watch.services
import be.alwyapps.watch.together.TogetherInbox
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Avatar
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.ConfirmDialog
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.DefaultAvatar
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalInitialFocusDone
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.ServerBadge
import be.alwyapps.watch.ui.Value
import be.alwyapps.watch.ui.focusOnAppear
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/**
 * One AlwyWatch account for a person's sign-ins (their Plex user and their Jellyfin user), see
 * `docs/identity-apps.md`: the sign-ins of my account, the device's accounts that could join
 * it, and the one-time "Are these both you?" offer.
 */
object Identity {
    /** The sign-ins of my AlwyWatch account as the server last said (null: not read yet). */
    var links by mutableStateOf<List<LinkedIdentity>?>(null)

    /** The device account offered by "Are these both you?" (shown once per run at most). */
    var offer by mutableStateOf<Account?>(null)

    /** The offer was looked for during this run. */
    private var looked = false

    /** Allow-listed server ids of the connected AlwyWatch server (by address), read once. */
    private var served: Pair<String, Set<String>>? = null

    suspend fun refreshLinks(core: AlwyWatch) {
        if (!runCatching { core.keepActive() }.getOrDefault(false)) {
            links = null
            return
        }
        runCatching { core.keepLinks() }.onSuccess { links = it }
    }

    private suspend fun servedIds(core: AlwyWatch): Set<String>? {
        val base = core.keepConnection()?.baseUrl ?: return null
        served?.takeIf { it.first == base }?.let { return it.second }
        return runCatching { core.keepServers(base).map { it.id }.toSet() }.getOrNull()?.also { served = base to it }
    }

    /**
     * The device's other accounts that can join my AlwyWatch account: on a server it serves,
     * not the synced account's server (one sign-in per server: the server refuses), not linked
     * yet, and with a token stored here. Network (allow-list, links) and Keystore reads: call it
     * off the first frame, never from composition.
     */
    suspend fun linkable(
        core: AlwyWatch,
        ctx: Context,
    ): List<Account> {
        val keep = core.keepConnection() ?: return emptyList()
        val accounts = runCatching { core.accounts() }.getOrDefault(emptyList())
        if (accounts.size < 2) return emptyList()
        val mine = accounts.firstOrNull { it.id == keep.accountId } ?: return emptyList()
        val ids = servedIds(core) ?: return emptyList()
        if (links == null) refreshLinks(core)
        val linkedServers = links?.map { it.serverId }?.toSet() ?: return emptyList()
        val candidates =
            accounts.filter { it.id != mine.id && it.serverId != mine.serverId && it.serverId in ids && it.serverId !in linkedServers }
        return withContext(Dispatchers.IO) { candidates.filter { keepToken(ctx, it) != null } }
    }

    /**
     * Makes [account] part of my AlwyWatch account (signed in as the synced account), then
     * refreshes everything that depends on who I am. Throws [CoreException] (see [linkProblem]).
     */
    suspend fun link(
        core: AlwyWatch,
        ctx: Context,
        account: Account,
    ) {
        val token =
            withContext(Dispatchers.IO) { keepToken(ctx, account) }
                ?: throw CoreException.Unauthorized("no token for ${account.serverName}")
        links = core.keepLink(account.id, token)
        afterChange(core)
    }

    /** Takes a sign-in out of my account; it becomes a new, empty one. */
    suspend fun unlink(
        core: AlwyWatch,
        identity: LinkedIdentity,
    ) {
        links = core.keepUnlink(identity.userKey)
        // This device's own sign-in went: it is a fresh account now (the welcome step shows).
        if (identity.current) MyAccount.linked(core) else afterChange(core)
    }

    /**
     * After a link or an unlink: my `Me` again (its user key may have changed: everything here
     * reads it from [MyAccount.me], rooms compare seats by the keys the server sends), the
     * links, the rooms and invitations, and the leaving-soon list (my votes merged).
     */
    private suspend fun afterChange(core: AlwyWatch) {
        MyAccount.refresh(core)
        refreshLinks(core)
        TogetherInbox.refresh(core)
        runCatching { core.refreshLeavingSoon(true) }
        be.alwyapps.watch.ui.ItemsChanged
            .bump()
    }

    // ----- "Not now", remembered on this device per pair ------------------------------------

    private fun prefs(ctx: Context) = ctx.getSharedPreferences("identity-${ctx.services.profile}", Context.MODE_PRIVATE)

    private fun pair(
        myKey: String,
        account: Account,
    ) = "$myKey|${account.id}"

    fun declined(
        ctx: Context,
        myKey: String,
        account: Account,
    ): Boolean = prefs(ctx).getStringSet("declined", emptySet())!!.contains(pair(myKey, account))

    fun decline(
        ctx: Context,
        myKey: String,
        account: Account,
    ) {
        val p = prefs(ctx)
        val set = p.getStringSet("declined", emptySet())!!.toMutableSet()
        set += pair(myKey, account)
        p.edit().putStringSet("declined", set).apply()
    }

    /** An account was signed out: once signed in again, it may be offered again. */
    fun forgetAccount(
        ctx: Context,
        accountId: String,
    ) {
        val p = prefs(ctx)
        val set = p.getStringSet("declined", emptySet())!!.filterNot { it.endsWith("|$accountId") }.toSet()
        p.edit().putStringSet("declined", set).apply()
        links = null
        looked = false
    }

    /** Looks once per run for a pair to offer (a few seconds after the start, never before). */
    suspend fun lookForOffer(
        core: AlwyWatch,
        ctx: Context,
    ) {
        if (looked) return
        // Most devices have one account: nothing to look up, no network.
        if (runCatching { core.accounts().size }.getOrDefault(0) < 2) return
        if (core.keepConnection() == null) return
        looked = true
        val me = MyAccount.me ?: return
        val pick = linkable(core, ctx).firstOrNull { !declined(ctx, me.userKey, it) } ?: return
        offer = pick
    }
}

/** A failed link or unlink in plain words. */
fun linkProblem(
    e: CoreException,
    serverName: String?,
): String {
    val status =
        (e as? CoreException.Server)
            ?.message
            ?.substringAfter("HTTP ", "")
            ?.takeWhile { it.isDigit() }
            ?.toIntOrNull()
    return when {
        e is CoreException.Unauthorized ->
            "Your ${serverName ?: "other"} sign-in has expired: sign in again on that account, then link."
        e is CoreException.Network -> "Can't reach your AlwyWatch server right now."
        status == 409 -> "These are two different people on the same server: they can't be one AlwyWatch account."
        status == 429 -> "Too many tries. Try again in a few minutes."
        status == 502 -> "The media server is unreachable right now. Try again later."
        else -> e.message ?: "Something went wrong."
    }
}

private fun kindLabel(kind: ServerKind) = if (kind == ServerKind.PLEX) "Plex" else "Jellyfin"

private fun kindLabel(kind: ServerType) = if (kind == ServerType.PLEX) "Plex" else "Jellyfin"

/**
 * "Are these both you?": the synced account (me) and [other], with names, servers and pictures
 * so a family member's account on a shared device is not linked by mistake. Link (focused
 * first) or Not now. TV: a full-screen dialog; phones and tablets: a dialog.
 */
@Composable
fun LinkDialog(
    other: Account,
    onDone: (linked: Boolean) -> Unit,
    onNotNow: () -> Unit,
    /** BACK: closed without an answer (not remembered as "Not now"). */
    onClose: () -> Unit = onNotNow,
) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    val me = MyAccount.me
    val mine = remember { core.keepConnection()?.accountId?.let { id -> runCatching { core.accounts() }.getOrNull()?.firstOrNull { it.id == id } } }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val nameA = mine?.userName ?: me?.mediaName?.ifEmpty { null } ?: me?.name ?: "this account"
    val serverA = mine?.let { "${it.serverName}, ${kindLabel(it.kind)}" } ?: ""
    val serverB = "${other.serverName}, ${kindLabel(other.kind)}"

    fun link() {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try {
                Identity.link(core, ctx, other)
                onDone(true)
            } catch (e: CoreException) {
                error = linkProblem(e, other.serverName)
            } finally {
                busy = false
            }
        }
    }

    @Composable
    fun Person(
        name: String,
        server: String,
        avatar: @Composable () -> Unit,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            avatar()
            Column {
                Text(name, style = MaterialTheme.typography.titleMedium)
                Text(server, style = MaterialTheme.typography.bodyMedium, color = Muted)
            }
        }
    }
    val size = if (d.isTv) 56.dp else 48.dp
    val body: @Composable () -> Unit = {
        Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text(
                "Use one AlwyWatch account for both of these sign-ins:",
                style = MaterialTheme.typography.bodyLarge,
            )
            Person(nameA, serverA) {
                if (me != null) Avatar(me.avatar, me.name, me.userKey, size) else DefaultAvatar(nameA, size)
            }
            Person(other.userName, serverB) { DefaultAvatar(other.userName, size) }
            Text(
                "They will share one name and picture, the same keep choices and the same watch-together rooms, on every device.",
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(
                "Only link them if both are you, not someone else who uses this device. " +
                    "It can't simply be undone: unlinking later turns that sign-in into a new, empty account.",
                style = MaterialTheme.typography.bodyMedium,
                color = Muted,
            )
            error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodyMedium) }
        }
    }
    val buttons: @Composable () -> Unit = {
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            ActionButton(if (busy) "Linking…" else "Link", { link() }, Modifier.focusOnAppear(), primary = true, icon = Icons.Check)
            ActionButton("Not now", onNotNow)
        }
    }
    if (d.isTv) {
        Dialog(
            onDismissRequest = onClose,
            properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false),
        ) {
            CompositionLocalProvider(LocalInitialFocusDone provides null) {
                Box(Modifier.fillMaxSize().background(Background), contentAlignment = Alignment.Center) {
                    Column(
                        Modifier
                            .widthIn(max = 720.dp)
                            .fillMaxWidth()
                            .verticalScroll(rememberScrollState())
                            .padding(horizontal = d.gutter, vertical = 32.dp),
                        verticalArrangement = Arrangement.spacedBy(22.dp),
                    ) {
                        Text("Are these both you?", style = MaterialTheme.typography.displaySmall)
                        body()
                        buttons()
                    }
                }
            }
        }
    } else {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = onClose,
            containerColor = Background,
            title = { Text("Are these both you?") },
            text = { Column(Modifier.verticalScroll(rememberScrollState())) { body() } },
            confirmButton = { ActionButton(if (busy) "Linking…" else "Link", { link() }, primary = true) },
            dismissButton = { ActionButton("Not now", onNotNow) },
        )
    }
}

/**
 * Over the app (MainActivity only, so never over the player): the one-time offer, looked for a
 * few seconds after the start (it adds nothing to it) and shown when no other prompt is.
 */
@Composable
fun IdentityPrompt() {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val keep = remember { core.keepConnection() }
    LaunchedEffect(keep?.baseUrl, keep?.accountId) {
        if (keep == null) return@LaunchedEffect
        // Well after the first frame and the screen's own loading.
        delay(5_000)
        // Who I am there (the welcome step reads it at start).
        snapshotFlow { MyAccount.me }.first { it != null }
        Identity.lookForOffer(core, ctx)
    }
    val other = Identity.offer ?: return
    if (welcomeShowing() || TogetherInbox.prompt != null) return
    LinkDialog(
        other,
        onDone = { Identity.offer = null },
        onNotNow = {
            MyAccount.me?.let { Identity.decline(ctx, it.userKey, other) }
            Identity.offer = null
        },
        // Asked again at a later start; Settings has it meanwhile.
        onClose = { Identity.offer = null },
    )
}

/** "Also signed in as …": the other sign-ins of my account, for a second line. */
fun alsoSignedInAs(links: List<LinkedIdentity>?): String? =
    links
        ?.filter { !it.current }
        ?.takeIf { it.isNotEmpty() }
        ?.joinToString(", ") { "${it.mediaName} (${it.serverName})" }
        ?.let { "Also signed in as $it" }

/**
 * Settings → AlwyWatch & requests → Your AlwyWatch account → Linked sign-ins: the
 * sign-ins of my account ("Main", "This device"), Unlink for the others, and "Link another
 * sign-in" when an account of this device can join. Shown when there is something to show.
 */
@Composable
fun LinkedSignInsGroup(me: Me) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var linkable by remember { mutableStateOf<List<Account>>(emptyList()) }
    var version by remember { mutableStateOf(0) }
    LaunchedEffect(me.userKey, version) {
        Identity.refreshLinks(core)
        linkable = Identity.linkable(core, ctx)
    }
    val links = Identity.links.orEmpty()
    if (links.size < 2 && linkable.isEmpty()) return
    var unlinking by remember { mutableStateOf<LinkedIdentity?>(null) }
    var choosing by remember { mutableStateOf(false) }
    var linking by remember { mutableStateOf<Account?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    unlinking?.let { id ->
        ConfirmDialog(
            "Unlink ${id.mediaName}?",
            "${id.mediaName} on ${id.serverName} becomes a separate, new AlwyWatch account with no votes, name or history. Continue?",
            confirm = "Unlink",
            onConfirm = {
                error = null
                scope.launch {
                    try {
                        Identity.unlink(core, id)
                        version++
                    } catch (e: CoreException) {
                        val status = e.message?.substringAfter("HTTP ", "")?.takeWhile { it.isDigit() }
                        error = if (status == "409") "The main sign-in can't be unlinked." else linkProblem(e, id.serverName)
                    }
                }
            },
            onDismiss = { unlinking = null },
        )
    }
    linking?.let { a ->
        LinkDialog(a, onDone = {
            linking = null
            version++
        }, onNotNow = { linking = null })
    }
    if (choosing) {
        be.alwyapps.watch.ui.ChoiceDialog(
            "Link another sign-in",
            linkable.map { it.id to "${it.userName} · ${it.serverName} (${kindLabel(it.kind)})" },
            null,
            onPick = { id ->
                choosing = false
                linking = linkable.firstOrNull { it.id == id }
            },
            onDismiss = { choosing = false },
            mark = { id -> ServerBadge(id, diameter = 28.dp) },
        )
    }
    Group(
        "Linked sign-ins",
        footer = "Your sign-ins on different servers, as one AlwyWatch account: the same name, picture, keep choices and rooms.",
    ) {
        links.forEachIndexed { i, id ->
            if (i > 0) Divider()
            ListRow(
                id.mediaName,
                subtitle =
                    listOfNotNull(
                        "${id.serverName} · ${kindLabel(id.kind)}",
                        "This device".takeIf { id.current },
                    ).joinToString(" · "),
                leading = { ServerBadge(id.userKey, diameter = if (LocalDimens.current.isTv) 40.dp else 34.dp) },
                // The main sign-in holds the account: it can't be unlinked.
                onClick = if (id.canonical) null else ({ unlinking = id }),
                trailing = { Value(if (id.canonical) "Main" else "Unlink") },
            )
        }
        if (linkable.isNotEmpty()) {
            if (links.isNotEmpty()) Divider()
            ListRow(
                "Link another sign-in",
                subtitle =
                    if (linkable.size == 1) {
                        "${linkable.single().userName} on ${linkable.single().serverName}"
                    } else {
                        "${linkable.size} accounts on this device"
                    },
                icon = Icons.Add,
                onClick = { if (linkable.size == 1) linking = linkable.single() else choosing = true },
                trailing = {
                    be.alwyapps.watch.ui
                        .Chevron()
                },
            )
        }
        error?.let { Text(it, Modifier.padding(16.dp), color = Danger, style = MaterialTheme.typography.bodySmall) }
    }
}
