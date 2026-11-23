// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.AccountInvite
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.Invitations
import be.alwyapps.watch.core.InviteItem
import be.alwyapps.watch.core.keep.ServerStatus
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.ChoiceRow
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
import be.alwyapps.watch.ui.OnSurface
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.ToggleRow
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import kotlinx.coroutines.launch

/**
 * Settings → Admin (admins of the AlwyWatch server only): people, the Leaving soon countdown,
 * kept forever, invitations, and what the server is connected to (read only).
 */
@Composable
fun AdminSection() {
    val nav = LocalNavigator.current
    ServerSettingsGroup()
    Group(tr("People")) {
        ListRow(
            tr("People"),
            subtitle = tr("What each person may keep, and whose request-site user they request as"),
            icon = Icons.Group,
            onClick = { nav.push(Route.AdminPeople) },
            trailing = { Chevron() },
        )
        Divider()
        ListRow(
            tr("Invitations"),
            subtitle = tr("Invite someone by email or with a link, also to Jellyfin"),
            icon = Icons.Add,
            onClick = { nav.push(Route.AdminInvites) },
            trailing = { Chevron() },
        )
    }
    Group(tr("Leaving soon")) {
        ListRow(
            tr("Leaving soon countdown"),
            subtitle = tr("Restart the deletion countdown of a collection"),
            icon = Icons.Schedule,
            onClick = { nav.push(Route.AdminCountdown) },
            trailing = { Chevron() },
        )
        Divider()
        ListRow(
            tr("Kept forever"),
            subtitle = tr("Per person, and how much of it someone else keeps too"),
            icon = Icons.FavoriteFilled,
            onClick = { nav.push(Route.KeepForever) },
            trailing = { Chevron() },
        )
    }
    ServerStatusGroups()
}

/** "{n} minutes" / "{n} hours" / "{n} days" since [at] (Unix seconds). */
private fun since(at: Long): String {
    val s = (System.currentTimeMillis() / 1000 - at).coerceAtLeast(0)
    return when {
        s < 3600 -> trn("{n} minute", "{n} minutes", (s / 60).coerceAtLeast(1))
        s < 86400 -> trn("{n} hour", "{n} hours", s / 3600)
        else -> trn("{n} day", "{n} days", s / 86400)
    }
}

/** What the AlwyWatch server is connected to, each line with what to change when it is off. */
@Composable
private fun ServerStatusGroups() {
    val core = LocalCore.current
    var status by remember { mutableStateOf<ServerStatus?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(Unit) {
        try {
            status = core.keepAdminStatus()
        } catch (e: CoreException) {
            error = e.message
        }
    }
    val st = status
    Group(tr("Server status"), footer = tr("Read only: change these in Server settings (web app).")) {
        when {
            st == null && error != null -> Text(error!!, Modifier.padding(16.dp), color = Danger, style = MaterialTheme.typography.bodySmall)
            st == null -> Text(tr("Checking…"), Modifier.padding(16.dp), color = Subtle)
            else -> {
                if (st.dryRun) {
                    Text(
                        tr("Test mode: nothing is changed in Maintainerr or Seerr. Turn it off in Server settings (web app)."),
                        Modifier.padding(16.dp),
                        color = Danger,
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    Divider()
                }

                @Composable
                fun Line(
                    title: String,
                    ok: Boolean,
                    text: String,
                ) = ListRow(
                    title,
                    subtitle = text,
                    icon = if (ok) Icons.Check else Icons.Info,
                    iconTint = if (ok) Accent else OnSurface,
                )
                Line(
                    "Maintainerr",
                    st.maintainerr == "ok",
                    when (st.maintainerr) {
                        "off" -> tr("Off. Set up Maintainerr in Server settings (web app).")
                        "starting" -> tr("Reading Maintainerr for the first time…")
                        "ok" ->
                            trn(
                                "Maintainerr {version} · read {time} ago · {n} title leaving",
                                "Maintainerr {version} · read {time} ago · {n} titles leaving",
                                st.leavingItems.toLong(),
                                "version" to st.maintainerrVersion.orEmpty(),
                                "time" to (st.lastSync?.let { since(it) } ?: "?"),
                            )
                        else -> tr("Can't reach Maintainerr. Check its address and API key in Server settings (web app).")
                    },
                )
                Divider()
                Line(
                    tr("Requests"),
                    st.seerrBridge,
                    if (st.seerrBridge) {
                        tr("On: every app requests as each person's Seerr user.")
                    } else {
                        tr("Off: only direct request-site logins work. Set up Seerr in Server settings (web app).")
                    },
                )
                Divider()
                Line(
                    "Jellyfin",
                    st.jellyfinKey,
                    if (st.jellyfinKey) {
                        tr("Keeps end by themselves once watched.")
                    } else {
                        tr("Jellyfin keeps don't end by themselves. Add the Jellyfin server's key in Server settings (web app).")
                    },
                )
                Divider()
                Line(
                    "Plex (Tautulli)",
                    st.tautulli,
                    if (st.tautulli) {
                        tr("Keeps end by themselves once watched.")
                    } else {
                        tr("Plex keeps don't end by themselves. Set up Tautulli in Server settings (web app).")
                    },
                )
                Divider()
                Line(
                    tr("Webhooks"),
                    st.webhooks,
                    if (st.webhooks) {
                        tr("On: watched titles are noticed at once.")
                    } else {
                        tr("Optional: watched titles are noticed within 15 min. For instant, add the webhooks shown in Server settings (web app).")
                    },
                )
                Divider()
                Line(tr("Passkeys"), st.passkeys, if (st.passkeys) tr("On") else tr("Off: set this site's address (https) in Server settings (web app)"))
                Divider()
                Line("Pocket ID", st.pocketid, if (st.pocketid) tr("On") else tr("Off"))
                Divider()
                Line(
                    tr("New accounts"),
                    st.registration != "closed",
                    when (st.registration) {
                        "open" -> tr("Anyone with the address")
                        "invite" -> tr("With an invitation")
                        else -> tr("Closed")
                    },
                )
                Divider()
                ListRow(tr("AlwyWatch server"), subtitle = st.version, icon = Icons.Hourglass)
            }
        }
    }
}

/** The AlwyWatch web app's Server settings page, or null when not connected to an AlwyWatch server. */
private fun serverSettingsPage(core: be.alwyapps.watch.core.AlwyWatch): String? =
    (core.keepConnection()?.baseUrl ?: core.account()?.baseUrl)?.let { it.trimEnd('/') + "/settings/admin/server" }

/** Opens Server settings in the browser (phone), or shows its address and QR code (TV). */
@Composable
private fun rememberServerSettingsOpener(): (() -> Unit)? {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val tv = LocalDimens.current.isTv
    val page = remember { serverSettingsPage(core) } ?: return null
    var showing by remember { mutableStateOf(false) }
    if (showing) WebPageDialog(tr("Server settings"), page) { showing = false }
    return { if (tv) showing = true else openInBrowser(ctx, page) }
}

/** Admin → Server settings: everything the AlwyWatch server connects to is set up in the web app. */
@Composable
private fun ServerSettingsGroup() {
    val open = rememberServerSettingsOpener() ?: return
    Group(tr("AlwyWatch server")) {
        ListRow(
            tr("Server settings"),
            subtitle = tr("Media servers, Seerr, Maintainerr, Tautulli, email and sign-up are set up in the web app."),
            icon = Icons.Settings,
            onClick = open,
            trailing = { Chevron() },
        )
    }
}

/** "12 Dec 2026" for Unix seconds, in the app's language. */
private fun day(at: Long): String =
    java.text.DateFormat
        .getDateInstance(java.text.DateFormat.MEDIUM, java.util.Locale.forLanguageTag(be.alwyapps.watch.ui.I18n.language))
        .format(java.util.Date(at * 1000))

/**
 * Settings → Admin → Invitations: invite someone by email (one use, sent by the server) or make
 * a link, optionally with a Jellyfin login that copies an existing user's access; then the
 * invitations, each can be ended. TV: every field and choice is reachable with the D-pad, OK on
 * a field opens the on-screen keyboard.
 */
@Composable
fun InvitationsScreen() {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val tv = d.isTv
    val scope = rememberCoroutineScope()
    val openSettings = rememberServerSettingsOpener()
    var data by remember { mutableStateOf<Invitations?>(null) }
    var loadError by remember { mutableStateOf<String?>(null) }
    var version by remember { mutableIntStateOf(0) }
    LaunchedEffect(version) {
        try {
            data = core.accountInvitations()
            loadError = null
        } catch (e: CoreException.NotFound) {
            loadError = tr("Invitations are made with an AlwyWatch account: sign in to yours on this device (Settings → Account), or use the web app.")
        } catch (e: CoreException) {
            loadError = e.message
        }
    }
    val inv = data
    val targets = inv?.jellyfin.orEmpty()

    var email by remember { mutableStateOf("") }
    var note by remember { mutableStateOf("") }
    var makeJellyfin by remember { mutableStateOf(true) }
    var target by remember { mutableStateOf<String?>(null) }
    // "" = Jellyfin's defaults; null = not chosen yet (the last one used, else the first user).
    var like by remember { mutableStateOf<String?>(null) }
    var days by remember { mutableLongStateOf(7) }
    var uses by remember { mutableLongStateOf(1) }
    var result by remember { mutableStateOf<AccountInvite?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var ending by remember { mutableStateOf<InviteItem?>(null) }

    val t = targets.firstOrNull { it.serverId == target } ?: targets.firstOrNull()
    val likeShown = like ?: t?.lastLike?.takeIf { l -> t.users.any { it.id == l } } ?: t?.users?.firstOrNull()?.id ?: ""
    val withEmail = inv?.mail == true && email.isNotBlank()

    fun send() {
        if (busy || inv == null) return
        busy = true
        error = null
        scope.launch {
            try {
                val jf = t?.takeIf { makeJellyfin }
                result =
                    core.accountInviteSend(
                        email.takeIf { withEmail },
                        jf?.serverId,
                        jf?.let { likeShown.ifEmpty { null } },
                        note.takeIf { withEmail },
                        days,
                        if (withEmail) 1 else uses,
                    )
                email = ""
                note = ""
                version++
            } catch (e: CoreException) {
                error = e.message
            } finally {
                busy = false
            }
        }
    }

    ending?.let { item ->
        ConfirmDialog(
            tr("End this invitation?"),
            tr("Its code and link stop working. Accounts already made with it stay."),
            confirm = tr("End"),
            onConfirm = {
                scope.launch {
                    try {
                        core.accountRevokeInvite(item.id)
                        version++
                    } catch (e: CoreException) {
                        error = e.message
                    }
                }
            },
            onDismiss = { ending = null },
        )
    }

    be.alwyapps.watch.ui.MinimalScrollSpec {
        Column(
            Modifier
                .fillMaxSize()
                .imePadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = d.gutter, vertical = 16.dp)
                .widthIn(max = 760.dp),
            verticalArrangement = Arrangement.spacedBy(26.dp),
        ) {
            PageTitle(tr("Invitations"), tr("A new person gets an AlwyWatch account, and a Jellyfin login if you like"))
            when {
                inv == null && loadError != null -> Text(loadError!!, color = Danger, style = MaterialTheme.typography.bodyMedium)
                inv == null -> Text(tr("Loading…"), color = Subtle)
                else -> {
                    Group(tr("Invite someone")) {
                        if (inv.mail) {
                            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Field(
                                    email,
                                    { email = it.trim() },
                                    label = { Text(tr("Email (optional)")) },
                                    placeholder = { Text("friend@example.org") },
                                    singleLine = true,
                                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Email, imeAction = ImeAction.Next),
                                    modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
                                )
                                Text(
                                    if (email.isBlank()) {
                                        tr("Empty: you get a link and a code to send them yourself.")
                                    } else {
                                        tr("AlwyWatch emails them a link that works once.")
                                    },
                                    color = Muted,
                                    style = MaterialTheme.typography.bodySmall,
                                )
                            }
                        } else {
                            ListRow(
                                tr("Set up email in Server settings to send invitations by email"),
                                subtitle = tr("For now, you get a link and a code to send them yourself."),
                                icon = Icons.Info,
                                onClick = openSettings,
                                trailing = openSettings?.let { { Chevron() } },
                                modifier = Modifier.initialFocus(),
                            )
                        }
                        if (t != null) {
                            Divider()
                            ToggleRow(
                                tr("Also make a Jellyfin login on {server}", "server" to t.name),
                                makeJellyfin,
                                { makeJellyfin = it },
                                subtitle = tr("Same username and password as their AlwyWatch account"),
                                icon = Icons.Library,
                            )
                            if (makeJellyfin) {
                                if (targets.size > 1) {
                                    Divider()
                                    ChoiceRow(
                                        tr("Jellyfin server"),
                                        targets.map { it.serverId to it.name },
                                        t.serverId,
                                        {
                                            target = it
                                            like = null
                                        },
                                    )
                                }
                                Divider()
                                ChoiceRow(
                                    tr("With the same access as"),
                                    t.users.map { it.id to it.name } + ("" to tr("Jellyfin's defaults")),
                                    likeShown,
                                    { like = it },
                                    subtitle = tr("Libraries, parental rating and permissions are copied from this user"),
                                )
                            }
                        }
                        if (withEmail) {
                            Divider()
                            Column(Modifier.padding(16.dp)) {
                                Field(
                                    note,
                                    { note = it.take(500) },
                                    label = { Text(tr("Personal note (optional)")) },
                                    placeholder = { Text(tr("A few words in the email")) },
                                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                                )
                            }
                        }
                        Divider()
                        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            Text(tr("Valid for"), color = Muted, style = MaterialTheme.typography.bodySmall)
                            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                for (n in listOf(1L, 7L, 30L)) ActionButton(trn("{n} day", "{n} days", n), { days = n }, selected = days == n)
                            }
                            if (!withEmail) {
                                Text(tr("How many people"), color = Muted, style = MaterialTheme.typography.bodySmall)
                                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    for (n in listOf(1L, 5L, 10L)) ActionButton(trn("{n} person", "{n} people", n), { uses = n }, selected = uses == n)
                                }
                            }
                        }
                    }
                    ActionButton(
                        when {
                            busy && withEmail -> tr("Sending…")
                            busy -> tr("Creating…")
                            withEmail -> tr("Send invitation")
                            else -> tr("Create link")
                        },
                        { send() },
                        primary = true,
                        icon = Icons.Add,
                    )
                    error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodyMedium) }
                    result?.let { InviteResult(it, tv) }
                    InvitationList(inv.invites) { ending = it }
                }
            }
        }
    }
}

/** What was just made: "Sent to …", or the code and link to pass on (Copy/Share; TV: a QR code). */
@Composable
private fun InviteResult(
    i: AccountInvite,
    tv: Boolean,
) {
    val ctx = LocalContext.current
    Group(null) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            i.sentTo?.let {
                Text(tr("Sent to {email}", "email" to it), style = MaterialTheme.typography.titleMedium, color = Accent)
            }
            Text(tr("Invitation code"), color = Muted, style = MaterialTheme.typography.bodySmall)
            Text(i.code, style = MaterialTheme.typography.headlineMedium, color = if (i.sentTo == null) Accent else OnSurface)
            i.url?.let { url ->
                Text(tr("Or the link"), color = Muted, style = MaterialTheme.typography.bodySmall)
                Text(url.substringAfter("://"), style = MaterialTheme.typography.titleSmall)
                if (tv && i.sentTo == null) QrImage(url)
            }
            Text(
                trn(
                    "Valid {n} day, for {count}. In AlwyWatch: Add a profile → Create an AlwyWatch account.",
                    "Valid {n} days, for {count}. In AlwyWatch: Add a profile → Create an AlwyWatch account.",
                    i.expiresInDays,
                    "count" to trn("{n} person", "{n} people", i.uses),
                ),
                color = Muted,
                style = MaterialTheme.typography.bodySmall,
            )
            if (!tv && i.sentTo == null) {
                val text = listOfNotNull(tr("Your AlwyWatch invitation code: {code}", "code" to i.code), i.url).joinToString("\n")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    ActionButton(tr("Copy"), {
                        val cm = ctx.getSystemService(android.content.Context.CLIPBOARD_SERVICE) as? android.content.ClipboardManager
                        cm?.setPrimaryClip(android.content.ClipData.newPlainText("AlwyWatch", i.url ?: i.code))
                    })
                    ActionButton(tr("Share"), {
                        runCatching {
                            ctx.startActivity(
                                Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text), null),
                            )
                        }
                    }, primary = true)
                }
            }
        }
    }
}

/** The invitations: to whom (or a link), the Jellyfin server, by whom, and what became of it. */
@Composable
private fun InvitationList(
    items: List<InviteItem>,
    onEnd: (InviteItem) -> Unit,
) {
    if (items.isEmpty()) return
    val now = System.currentTimeMillis() / 1000
    Group(tr("Invitations")) {
        items.forEachIndexed { n, it ->
            if (n > 0) Divider()
            val active = it.usesLeft > 0 && it.expires > now
            val status =
                when {
                    active -> tr("Until {date}", "date" to day(it.expires))
                    it.usedBy.isEmpty() -> tr("Ended")
                    else -> null
                }
            val parts =
                listOfNotNull(
                    it.jellyfin?.let { j -> tr("with a login on {server}", "server" to j) },
                    tr("by {name}", "name" to it.createdBy),
                    it.usedBy.takeIf { u -> u.isNotEmpty() }?.let { u -> tr("used by {names}", "names" to u.joinToString(", ")) },
                    status,
                )
            ListRow(
                it.email ?: tr("Link"),
                subtitle = parts.joinToString(" · "),
                icon = if (it.email != null) Icons.Person else Icons.Add,
                iconTint = if (active) Accent else Subtle,
                trailing = if (active) ({ ActionButton(tr("End"), { onEnd(it) }) }) else null,
            )
        }
    }
}
