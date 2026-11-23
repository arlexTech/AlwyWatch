// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.AccountInvite
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.ServerStatus
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.OnSurface
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.Subtle
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
    var inviting by remember { mutableStateOf(false) }
    if (inviting) InvitationDialog { inviting = false }
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
            subtitle = tr("A code and a link to create an AlwyWatch account"),
            icon = Icons.Add,
            onClick = { inviting = true },
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
    Group(tr("Server status"), footer = tr("Read only: these are set in the server's config.toml and .env, then the server is restarted.")) {
        when {
            st == null && error != null -> Text(error!!, Modifier.padding(16.dp), color = Danger, style = MaterialTheme.typography.bodySmall)
            st == null -> Text(tr("Checking…"), Modifier.padding(16.dp), color = Subtle)
            else -> {
                if (st.dryRun) {
                    Text(
                        tr("Test mode: nothing is changed in Maintainerr or Seerr. Remove ALWYWATCH_DRY_RUN from .env and restart."),
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
                        "off" -> tr("Off. Add a [maintainerr] block (url, server) to config.toml and restart.")
                        "starting" -> tr("Reading Maintainerr for the first time…")
                        "ok" ->
                            trn(
                                "Maintainerr {version} · read {time} ago · {n} title leaving",
                                "Maintainerr {version} · read {time} ago · {n} titles leaving",
                                st.leavingItems.toLong(),
                                "version" to st.maintainerrVersion.orEmpty(),
                                "time" to (st.lastSync?.let { since(it) } ?: "?"),
                            )
                        else -> tr("Can't reach Maintainerr. Check [maintainerr] url, the Docker network, and MAINTAINERR_API_KEY if it uses one.")
                    },
                )
                Divider()
                Line(
                    tr("Requests"),
                    st.seerrBridge,
                    if (st.seerrBridge) {
                        tr("On: every app requests as each person's Seerr user.")
                    } else {
                        tr("Off: only direct request-site logins work. Turn on with [seerr] in config.toml and SEERR_API_KEY in .env.")
                    },
                )
                Divider()
                Line(
                    "Jellyfin",
                    st.jellyfinKey,
                    if (st.jellyfinKey) tr("Keeps end by themselves once watched.") else tr("Jellyfin keeps don't end by themselves. Set JELLYFIN_API_KEY."),
                )
                Divider()
                Line(
                    "Plex (Tautulli)",
                    st.tautulli,
                    if (st.tautulli) {
                        tr("Keeps end by themselves once watched.")
                    } else {
                        tr("Plex keeps don't end by themselves. Add [tautulli] and TAUTULLI_API_KEY.")
                    },
                )
                Divider()
                Line(
                    tr("Webhooks"),
                    st.webhooks,
                    if (st.webhooks) {
                        tr("On: watched titles are noticed at once.")
                    } else {
                        tr("Optional: watched titles are noticed within 15 min. For instant, set WEBHOOK_SECRET.")
                    },
                )
                Divider()
                Line(tr("Passkeys"), st.passkeys, if (st.passkeys) tr("On") else tr("Off: set ALWYWATCH_PUBLIC_URL (https)"))
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

/**
 * A new invitation: for how long and how many people, then its code and link (phones can
 * share it; TV shows both to type elsewhere).
 */
@Composable
private fun InvitationDialog(onDismiss: () -> Unit) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val tv = LocalDimens.current.isTv
    val scope = rememberCoroutineScope()
    var days by remember { mutableLongStateOf(7) }
    var uses by remember { mutableLongStateOf(1) }
    var invite by remember { mutableStateOf<AccountInvite?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }

    fun create() {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try {
                invite = core.accountInvite(days, uses)
            } catch (e: CoreException.NotFound) {
                error = tr("Invitations are made with an AlwyWatch account: sign in to yours on this device (Settings → Account), or use the web app.")
            } catch (e: CoreException) {
                error = e.message
            } finally {
                busy = false
            }
        }
    }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Background,
        title = { Text(tr("New invitation")) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                val i = invite
                if (i == null) {
                    Text(tr("Valid for"), color = Muted, style = MaterialTheme.typography.bodySmall)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for (n in listOf(1L, 7L, 30L)) ActionButton(trn("{n} day", "{n} days", n), { days = n }, selected = days == n)
                    }
                    Text(tr("For"), color = Muted, style = MaterialTheme.typography.bodySmall)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        for (n in listOf(1L, 5L, 10L)) ActionButton(trn("{n} person", "{n} people", n), { uses = n }, selected = uses == n)
                    }
                } else {
                    Text(tr("Invitation code"), color = Muted, style = MaterialTheme.typography.bodySmall)
                    Text(i.code, style = MaterialTheme.typography.displaySmall, color = Accent)
                    i.url?.let { url ->
                        Text(tr("Or the link"), color = Muted, style = MaterialTheme.typography.bodySmall)
                        Text(url.substringAfter("://"), style = MaterialTheme.typography.titleMedium)
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
                }
                error?.let { Text(it, color = Danger, style = MaterialTheme.typography.bodySmall) }
            }
        },
        confirmButton = {
            val i = invite
            when {
                i == null -> ActionButton(if (busy) tr("Creating…") else tr("Create"), { create() }, primary = true)
                !tv ->
                    ActionButton(tr("Share"), {
                        val text = listOfNotNull(tr("Your AlwyWatch invitation code: {code}", "code" to i.code), i.url).joinToString("\n")
                        runCatching {
                            ctx.startActivity(
                                Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text), null),
                            )
                        }
                    }, primary = true)
                else -> ActionButton(tr("Done"), onDismiss, primary = true)
            }
        },
        dismissButton = { ActionButton(if (invite == null) tr("Cancel") else tr("Close"), onDismiss) },
    )
}
