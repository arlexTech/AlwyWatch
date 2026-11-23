// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.BuildConfig
import be.alwyapps.watch.core.coreVersion
import be.alwyapps.watch.core.media.LibraryKind
import be.alwyapps.watch.core.media.ServerKind
import be.alwyapps.watch.player.PlayerSettings
import be.alwyapps.watch.player.languageName
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.ChoiceRow
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.I18n
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.Images
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.PageTitle
import be.alwyapps.watch.ui.Route
import be.alwyapps.watch.ui.ToggleRow
import be.alwyapps.watch.ui.UiPrefs
import be.alwyapps.watch.ui.Value
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import be.alwyapps.watch.ui.usesRail

/** A page of Settings, opened from the list (`Route.SettingsSection`). Same order on every app. */
private enum class Section(
    val id: String,
    val title: String,
    val icon: ImageVector,
) {
    ACCOUNT("account", "Account", Icons.Person),
    SERVERS("servers", "Servers", Icons.Library),
    REQUESTS("requests", "Requests", Icons.Add),
    PLAYBACK("playback", "Playback", Icons.Play),
    AUDIO("audio", "Audio & subtitles", Icons.Subtitles),
    INTERFACE("interface", "Menu & pages", Icons.Home),
    DOWNLOADS("downloads", "Downloads", Icons.Download),
    DEVICE("device", "This device", Icons.Tv),
    ADMIN("admin", "Admin", Icons.Settings),
    ;

    companion object {
        /** Older ids (a back stack saved by an earlier version) open where their rows went. */
        fun of(id: String) =
            when (id) {
                "services", "alwywatch" -> ACCOUNT
                "about", "profiles" -> DEVICE
                else -> entries.firstOrNull { it.id == id } ?: DEVICE
            }
    }
}

/** The section's title in the selected language ([Section.title] stays English: a key). */
private fun Section.label(): String =
    when (this) {
        Section.ACCOUNT -> tr("Account")
        Section.SERVERS -> tr("Servers")
        Section.REQUESTS -> tr("Requests")
        Section.PLAYBACK -> tr("Playback")
        Section.AUDIO -> tr("Audio & subtitles")
        Section.INTERFACE -> tr("Menu & pages")
        Section.DOWNLOADS -> tr("Downloads")
        Section.DEVICE -> tr("This device")
        Section.ADMIN -> tr("Admin")
    }

/** "5 seconds" for a choice list. */
private fun seconds(n: Int) = trn("{n} second", "{n} seconds", n)

private fun kindName(kind: ServerKind) = if (kind == ServerKind.PLEX) "Plex" else "Jellyfin"

/** The scrolling column every Settings page uses. */
@Composable
private fun SettingsPage(
    title: String,
    /** English, whatever the language: the key of the remembered row. */
    key: String,
    content: @Composable () -> Unit,
) {
    val d = LocalDimens.current
    // Back on this page: the row that was opened gets the focus (else its first row, TV).
    // TV: rows scroll into view just enough (the default keeps the focused row a third of the
    // way down: a page opening on a row under its first group had its title half off screen).
    androidx.compose.runtime.CompositionLocalProvider(be.alwyapps.watch.ui.LocalRowMemory provides "settings.$key") {
        be.alwyapps.watch.ui.MinimalScrollSpec {
            Column(
                Modifier
                    .fillMaxSize()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = d.gutter, vertical = 16.dp)
                    .widthIn(max = 760.dp),
                verticalArrangement = Arrangement.spacedBy(26.dp),
            ) {
                PageTitle(title)
                content()
            }
        }
    }
}

/** Settings: one row per section, each saying what is set there now. */
@Composable
fun SettingsScreen(
    @Suppress("UNUSED_PARAMETER") onAccountChanged: (String?) -> Unit,
) {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    val d = LocalDimens.current
    val ctx = LocalContext.current
    val settings = remember { PlayerSettings(core) }
    val accounts = remember { core.accounts() }
    val keep = remember { core.keepConnection() }
    val account = remember { core.account() }
    val seerr = remember { core.seerrConnection() }
    val rail = usesRail()
    androidx.compose.runtime.LaunchedEffect(Unit) { MyAccount.refresh(core) }
    val admin = keep?.isAdmin == true || MyAccount.me?.isAdmin == true

    fun on(b: Boolean) = if (b) tr("on") else tr("off")

    fun summary(s: Section): String =
        when (s) {
            Section.ACCOUNT -> {
                val address = (keep?.baseUrl ?: account?.baseUrl)?.substringAfter("://")?.trimEnd('/')
                if (address == null) {
                    tr("No AlwyWatch account · connect to AlwyWatch")
                } else {
                    listOfNotNull(MyAccount.me?.name ?: account?.name ?: keep?.userName, address).joinToString(" · ")
                }
            }
            Section.SERVERS ->
                accounts.joinToString(" · ") { "${it.serverName} (${kindName(it.kind)})" }.ifEmpty { tr("No server yet") }
            Section.REQUESTS ->
                when {
                    seerr == null -> tr("Off")
                    seerr.viaKeep -> tr("Through your AlwyWatch server")
                    else -> tr("Request site {address}", "address" to seerr.baseUrl.substringAfter("://").trimEnd('/'))
                }
            Section.PLAYBACK ->
                tr(
                    "Skip intros {intros} · next episode {next} · −{back} s / +{forward} s",
                    "intros" to on(settings.autoSkipIntro),
                    "next" to on(settings.autoPlayNext),
                    "back" to settings.seekBackS,
                    "forward" to settings.seekForwardS,
                )
            Section.AUDIO ->
                listOf(
                    languageName(settings.audioLanguage.ifEmpty { null }) ?: tr("Original audio"),
                    when (settings.subtitleMode) {
                        "forced" -> tr("subtitles forced only")
                        "always" -> tr("subtitles always")
                        "off" -> tr("subtitles off")
                        else -> tr("subtitles automatic")
                    },
                    when (settings.subtitleSize) {
                        "small" -> tr("small")
                        "normal" -> tr("normal")
                        "large" -> tr("large")
                        "huge" -> tr("huge")
                        else -> settings.subtitleSize
                    },
                ).joinToString(" · ")
            Section.INTERFACE ->
                listOfNotNull(
                    tr("libraries in the menu").takeIf { rail },
                    tr("clock").takeIf { d.isTv },
                    tr("theme music"),
                    tr("quitting"),
                ).joinToString(", ").replaceFirstChar { it.uppercase() }
            Section.DOWNLOADS -> tr("When to delete, manage downloads")
            Section.DEVICE ->
                listOfNotNull(
                    trn(
                        "{n} profile",
                        "{n} profiles",
                        ctx.services.profiles
                            .list()
                            .size,
                    ),
                    I18n
                        .languages()
                        .firstOrNull { it.first == I18n.language }
                        ?.second,
                    "AlwyWatch ${BuildConfig.VERSION_NAME}",
                ).joinToString(" · ")
            Section.ADMIN -> tr("People, countdown, invitations, server status")
        }

    SettingsPage(tr("Settings"), "Settings") {
        Group(null) {
            Section.entries
                .filter { (it != Section.DOWNLOADS || !d.isTv) && (it != Section.ADMIN || admin) }
                .forEachIndexed { i, s ->
                    if (i > 0) Divider()
                    ListRow(
                        s.label(),
                        subtitle = summary(s),
                        icon = s.icon,
                        onClick = { nav.push(Route.SettingsSection(s.id)) },
                        trailing = { Chevron() },
                    )
                }
        }
    }
}

/** One section of Settings on its own page. */
@Composable
fun SettingsSectionScreen(
    id: String,
    onAccountChanged: (String?) -> Unit,
) {
    val s = Section.of(id)
    SettingsPage(s.label(), s.title) {
        when (s) {
            Section.ACCOUNT -> AccountSection()
            Section.SERVERS -> ServersSection(onAccountChanged)
            Section.REQUESTS -> {
                var version by remember { mutableIntStateOf(0) }
                RequestsGroup(version) { version++ }
            }
            Section.PLAYBACK -> PlaybackSection()
            Section.AUDIO -> AudioSection()
            Section.INTERFACE -> InterfaceSection()
            Section.DOWNLOADS -> DownloadsSection()
            Section.DEVICE -> {
                ProfilesGroup()
                OtherDevicesGroup()
                AboutSection()
            }
            Section.ADMIN -> AdminSection()
        }
    }
}

/**
 * Account: with AlwyWatch, how others see you, the AlwyWatch server, password and passkeys,
 * linked logins and signing out; without, this profile's name and "Connect to AlwyWatch".
 */
@Composable
private fun AccountSection() {
    val core = LocalCore.current
    val d = LocalDimens.current
    var version by remember { mutableIntStateOf(0) }
    if (!remember(version) { hasAlwyWatch(core) }) {
        LocalProfileGroups()
        return
    }
    val keep = remember(version) { core.keepConnection() }
    val account = remember(version) { core.account() }
    if (keep != null) {
        AlwyWatchAccountGroup()
    } else if (account != null) {
        // Signed in to the account only (no login on a server it serves yet).
        Group(tr("Your AlwyWatch account")) {
            ListRow(
                account.name,
                subtitle = tr("How others see you"),
                leading = {
                    be.alwyapps.watch.ui
                        .DefaultAvatar(account.name, if (d.isTv) 52.dp else 46.dp)
                },
            )
        }
    }
    AlwyWatchConnectionGroup(version) { version++ }
}

/** This device: sending this profile's logins and settings to a new device (AlwyWatch profiles). */
@Composable
private fun OtherDevicesGroup() {
    val core = LocalCore.current
    if (!remember { hasAlwyWatch(core) }) return
    var adding by remember { mutableStateOf(false) }
    if (adding) SendToDeviceDialog { adding = false }
    Group(tr("Other devices")) {
        ListRow(
            tr("Sign in another device"),
            subtitle = tr("On the new phone, tablet or TV, choose “Sign in with a code”, then type its code here"),
            icon = Icons.Add,
            onClick = { adding = true },
            trailing = { Chevron() },
        )
    }
}

@Composable
private fun ServersSection(onAccountChanged: (String?) -> Unit) {
    val core = LocalCore.current
    val current = LocalAccount.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    var version by remember { mutableIntStateOf(0) }
    val accounts = remember(version) { core.accounts() }
    var removing by remember { mutableStateOf<be.alwyapps.watch.core.Account?>(null) }
    removing?.let { a ->
        be.alwyapps.watch.ui.ConfirmDialog(
            tr("Sign out of {server}?", "server" to a.serverName),
            tr("{name} is signed out of {server} on this device. Its downloads stay until you delete them.", "name" to a.userName, "server" to a.serverName),
            confirm = tr("Sign out"),
            onConfirm = {
                core.removeAccount(a.id)
                ctx.services.tokens.remove(a.id)
                // Signed in again later, "Are these both you?" may ask again for it.
                Identity.forgetAccount(ctx, a.id)
                version++
                if (a.id == current) onAccountChanged(core.accounts().firstOrNull()?.id)
            },
            onDismiss = { removing = null },
        )
    }
    // Several servers: shown together (one preferred for titles on both) or one at a time.
    var together by remember { mutableStateOf(core.getSetting(COMBINED_KEY) != "0") }
    Group(
        tr("Your servers"),
        // The login shown now is also the one AlwyWatch uses (when the AlwyWatch server serves it).
        footer = if (accounts.size > 1 && hasAlwyWatch(core)) tr("AlwyWatch uses the login showing now.") else null,
    ) {
        // Together: the preferred account is also the one in use (search, leaving soon…).
        val main = if (together && accounts.size > 1) core.getSetting(PREFERRED_KEY)?.takeIf { p -> accounts.any { it.id == p } } ?: current else current
        accounts.forEachIndexed { i, a ->
            if (i > 0) Divider()
            ListRow(
                tr("{name} on {server}", "name" to a.userName, "server" to a.serverName),
                subtitle = kindName(a.kind),
                // Plex or Jellyfin, at a glance.
                leading = {
                    be.alwyapps.watch.ui
                        .ServerBadge(a.id, diameter = if (d.isTv) 40.dp else 34.dp)
                },
                onClick =
                    if (a.id != main) {
                        {
                            core.setSetting(PREFERRED_KEY, a.id)
                            core.setSetting("current_account", a.id)
                            onAccountChanged(a.id)
                        }
                    } else {
                        null
                    },
                trailing = {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (a.id == main) Value(if (together && accounts.size > 1) tr("Preferred") else tr("Showing now"))
                        if (a.kind == ServerKind.PLEX) {
                            IconAction(Icons.Person, tr("Switch Plex user"), { nav.push(Route.PlexSwitch(a.id)) })
                        }
                        IconAction(Icons.Close, tr("Sign out of {server}", "server" to a.serverName), { removing = a })
                    }
                },
            )
        }
        Divider()
        ListRow(tr("Add a server"), icon = Icons.Add, onClick = { nav.push(Route.AddServer) }, trailing = { Chevron() })
    }
    if (accounts.size > 1) {
        Group(tr("Home"), footer = tr("Together, the home mixes every server; a title on several is shown once, from the preferred one.")) {
            ToggleRow(
                tr("Show all servers together"),
                together,
                {
                    together = it
                    core.setSetting(COMBINED_KEY, if (it) "1" else "0")
                },
                subtitle =
                    if (together) {
                        if (d.isTv) {
                            tr("Select a login above to make it the preferred one")
                        } else {
                            tr("Tap a login above to make it the preferred one")
                        }
                    } else {
                        if (d.isTv) {
                            tr("One server at a time: select a login above to switch")
                        } else {
                            tr("One server at a time: tap a login above to switch")
                        }
                    },
            )
        }
        SyncGroup(accounts)
        Group(tr("Moving between servers"), footer = tr("A one-time copy, one way. Each person copies their own history, with their own logins.")) {
            ListRow(
                tr("Copy watch history"),
                subtitle = tr("What you watched and where you stopped, from one server to another (Plex to Jellyfin)"),
                icon = Icons.Refresh,
                onClick = { nav.push(Route.HistoryCopy) },
                trailing = { Chevron() },
            )
        }
    }
}

/**
 * Keep two servers in sync: one switch per pair of servers, with when it last ran and what it
 * changed. Turning one on syncs at once, then every 6 hours; watching in AlwyWatch is mirrored
 * right away.
 */
@Composable
private fun SyncGroup(accounts: List<be.alwyapps.watch.core.Account>) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val state = be.alwyapps.watch.history.SyncState
    val version = state.version
    var pairs by remember(version) { mutableStateOf(runCatching { core.syncPairs() }.getOrDefault(emptyList())) }
    // Every pair of accounts on different servers.
    val candidates =
        accounts.flatMapIndexed { i, a -> accounts.drop(i + 1).filter { it.serverId != a.serverId }.map { a to it } }
    if (candidates.isEmpty()) return
    Group(
        tr("Keep in sync"),
        footer =
            tr(
                "Both ways: what you watch, unwatch or where you stop on one server is copied to the other. Right away for what you watch in AlwyWatch, every 6 hours for other apps. Changed on both since the last sync: the further one wins.",
            ),
    ) {
        candidates.forEachIndexed { i, (a, b) ->
            if (i > 0) Divider()
            val on = pairs.any { (it.a == a.id && it.b == b.id) || (it.a == b.id && it.b == a.id) }
            val status = remember(version, on) { runCatching { core.syncStatus(a.id, b.id) }.getOrNull() }
            val subtitle =
                when {
                    !on ->
                        tr(
                            "{server} ({kind}) and {other} ({otherKind})",
                            "server" to a.serverName,
                            "kind" to kindName(a.kind),
                            "other" to b.serverName,
                            "otherKind" to kindName(b.kind),
                        )
                    state.running -> tr("Syncing now…")
                    status?.error != null -> tr("Last sync failed: {reason}", "reason" to status.error)
                    status?.lastRunAt != null -> {
                        val at =
                            java.text.DateFormat
                                .getTimeInstance(java.text.DateFormat.SHORT, java.util.Locale.forLanguageTag(I18n.language))
                                .format(java.util.Date(status.lastRunAt!! * 1000))
                        val changed = status.changes.toInt()
                        if (changed == 0) {
                            tr("Last synced {time} · nothing to change", "time" to at)
                        } else {
                            tr("Last synced {time} · {count} updated", "time" to at, "count" to changed)
                        }
                    }
                    else -> tr("Waiting for the first sync")
                }
            ToggleRow(
                "${a.serverName} ⇄ ${b.serverName}",
                on,
                { want ->
                    runCatching { core.setSyncPair(a.id, b.id, want) }
                    pairs = runCatching { core.syncPairs() }.getOrDefault(emptyList())
                    be.alwyapps.watch.history.SyncWorker
                        .schedule(ctx, core)
                    if (want) {
                        be.alwyapps.watch.history.SyncWorker
                            .now(ctx)
                    }
                },
                subtitle = subtitle,
            )
            if (on && status?.recent?.isNotEmpty() == true && !state.running) {
                Text(
                    status.recent.take(5).joinToString("\n"),
                    Modifier.padding(horizontal = 16.dp, vertical = 4.dp),
                    style = androidx.compose.material3.MaterialTheme.typography.bodySmall,
                    color = be.alwyapps.watch.ui.Muted,
                )
            }
        }
        if (pairs.isNotEmpty()) {
            Divider()
            ListRow(
                if (state.running) tr("Syncing…") else tr("Sync now"),
                icon = Icons.Refresh,
                onClick = {
                    if (!state.running) {
                        be.alwyapps.watch.history.SyncWorker
                            .now(ctx)
                    }
                },
            )
        }
    }
}

/** A choice of seconds, stored as an Int setting. */
@Composable
private fun SecondsRow(
    title: String,
    options: List<Pair<String, String>>,
    get: () -> Int,
    set: (Int) -> Unit,
    subtitle: String? = null,
) {
    var v by remember { mutableStateOf(get().toString()) }
    ChoiceRow(title, options, v, {
        v = it
        set(it.toInt())
    }, subtitle = subtitle)
}

@Composable
private fun PlaybackSection() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val settings = remember { PlayerSettings(core) }
    Group(tr("Intros and credits")) {
        var autoSkip by remember { mutableStateOf(settings.autoSkipIntro) }
        ToggleRow(
            tr("Skip intros automatically"),
            autoSkip,
            {
                autoSkip = it
                settings.autoSkipIntro = it
            },
            subtitle = tr("Otherwise a Skip intro button appears."),
        )
        Divider()
        var autoNext by remember { mutableStateOf(settings.autoPlayNext) }
        ToggleRow(
            tr("Play the next episode automatically"),
            autoNext,
            {
                autoNext = it
                settings.autoPlayNext = it
            },
            subtitle = tr("A countdown during the credits, which Back cancels. Off: playback stops at the end of the episode."),
        )
        Divider()
        SecondsRow(
            tr("Next episode countdown"),
            listOf("5" to seconds(5), "10" to seconds(10), "15" to seconds(15), "30" to seconds(30)),
            { settings.countdownS },
            { settings.countdownS = it },
        )
    }
    Group(tr("Remote and resuming")) {
        SecondsRow(
            tr("Skip back"),
            listOf("5" to seconds(5), "10" to seconds(10), "15" to seconds(15), "30" to seconds(30)),
            { settings.seekBackS },
            { settings.seekBackS = it },
        )
        Divider()
        SecondsRow(
            tr("Skip forward"),
            listOf("10" to seconds(10), "15" to seconds(15), "30" to seconds(30), "60" to trn("{n} minute", "{n} minutes", 1)),
            { settings.seekForwardS },
            { settings.seekForwardS = it },
        )
        Divider()
        SecondsRow(
            tr("Rewind when resuming"),
            listOf("0" to tr("Off"), "5" to seconds(5), "10" to seconds(10), "30" to seconds(30)),
            { settings.resumeRewindS },
            { settings.resumeRewindS = it },
        )
        Divider()
        SecondsRow(
            tr("Are you still watching?"),
            listOf("0" to tr("Never")) + listOf(2, 3, 4).map { "$it" to trn("After {n} hour", "After {n} hours", it) },
            { settings.stillWatchingH },
            { settings.stillWatchingH = it },
            subtitle = tr("Pauses when the remote has not been touched for that long."),
        )
    }
    Group(tr("Picture")) {
        var matchRate by remember { mutableStateOf(settings.matchFrameRate) }
        ToggleRow(
            tr("Match display refresh rate"),
            matchRate,
            {
                matchRate = it
                settings.matchFrameRate = it
            },
            subtitle = tr("Switches the TV to the video's frame rate (no judder)."),
        )
        // Tunneling only exists on TV devices.
        if (d.isTv) {
            Divider()
            var tunneling by remember { mutableStateOf(settings.tunneling) }
            ToggleRow(
                tr("Tunneled playback"),
                tunneling,
                {
                    tunneling = it
                    settings.tunneling = it
                },
                subtitle = tr("Lower power on some TV devices, but breaks playback on others."),
            )
        }
    }
    if (core.keepActive()) TogetherRightsGroup()
}

/** In watch together rooms I create, what the others may do (follows me to my other devices). */
@Composable
private fun TogetherRightsGroup() {
    val core = LocalCore.current
    var rights by remember { mutableStateOf(core.togetherRights()) }

    fun change(r: be.alwyapps.watch.core.keep.TogetherRights) {
        rights = r
        runCatching { core.setTogetherRights(r) }
    }
    Group(tr("Watch together")) {
        Text(
            tr("In rooms you create, what the others may do. You can always do everything."),
            Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        ToggleRow(
            tr("Others can change what's playing"),
            rights.changeTitle,
            { change(rights.copy(changeTitle = it)) },
            subtitle = tr("Next episode, another title."),
        )
        Divider()
        ToggleRow(tr("Others can pause and play"), rights.playPause, { change(rights.copy(playPause = it)) })
        Divider()
        ToggleRow(tr("Others can move forward or back"), rights.seek, { change(rights.copy(seek = it)) })
    }
}

@Composable
private fun AudioSection() {
    val core = LocalCore.current
    val settings = remember { PlayerSettings(core) }
    val languages =
        listOf("eng", "fra", "nld", "deu", "spa", "ita", "por", "jpn", "kor", "zho", "ara", "rus", "pol", "swe", "tur", "hin")
            .map { it to (languageName(it) ?: it) }
    Group(
        tr("Languages"),
        footer = tr("A movie or show remembers the tracks picked for it (on its page or while watching), and its next episodes follow."),
    ) {
        var audio by remember { mutableStateOf(settings.audioLanguage) }
        ChoiceRow(tr("Audio language"), listOf("" to tr("Original (the file's default)")) + languages, audio, {
            audio = it
            settings.audioLanguage = it
        })
        Divider()
        var mode by remember { mutableStateOf(settings.subtitleMode) }
        ChoiceRow(
            tr("Subtitles"),
            listOf("auto" to tr("Automatic"), "forced" to tr("Only foreign-language parts"), "always" to tr("Always"), "off" to tr("Off")),
            mode,
            {
                mode = it
                settings.subtitleMode = it
            },
            subtitle = tr("Automatic: when the audio is not in your subtitle language."),
        )
        Divider()
        var subLang by remember { mutableStateOf(settings.subtitleLanguage) }
        val device =
            languageName(
                java.util.Locale
                    .getDefault()
                    .language,
            ) ?: "?"
        ChoiceRow(tr("Subtitle language"), listOf("" to tr("Device language ({language})", "language" to device)) + languages, subLang, {
            subLang = it
            settings.subtitleLanguage = it
        })
    }
    Group(tr("Subtitle look")) {
        var size by remember { mutableStateOf(settings.subtitleSize) }
        ChoiceRow(tr("Size"), listOf("small" to tr("Small"), "normal" to tr("Normal"), "large" to tr("Large"), "huge" to tr("Huge")), size, {
            size = it
            settings.subtitleSize = it
        })
        Divider()
        var color by remember { mutableStateOf(settings.subtitleColor) }
        ChoiceRow(tr("Colour"), listOf("white" to tr("White"), "yellow" to tr("Yellow")), color, {
            color = it
            settings.subtitleColor = it
        })
        Divider()
        var box by remember { mutableStateOf(settings.subtitleBackground) }
        ToggleRow(tr("Background box"), box, {
            box = it
            settings.subtitleBackground = it
        }, subtitle = tr("Otherwise a dark outline keeps them readable."))
        Divider()
        var raised by remember { mutableStateOf(settings.subtitleRaised) }
        ToggleRow(tr("Raise subtitles"), raised, {
            raised = it
            settings.subtitleRaised = it
        }, subtitle = tr("Higher on the screen (TVs that crop the edges)."))
    }
}

@Composable
private fun InterfaceSection() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val settings = remember { PlayerSettings(core) }
    val prefs = remember { UiPrefs(core, d.isTv) }
    if (usesRail()) {
        Group(
            tr("Libraries in the menu"),
            footer =
                if (d.isTv) {
                    tr("Select a library to show or hide it; arrows change the order.")
                } else {
                    tr("Tap a library to show or hide it; arrows change the order.")
                },
        ) {
            val account = LocalAccount.current
            val libs =
                remember(account) {
                    runCatching { core.cachedHome(account)?.libraries }
                        .getOrNull()
                        .orEmpty()
                        .filter { it.kind != LibraryKind.OTHER }
                }
            var order by remember { mutableStateOf(prefs.arrange(libs, { it.id }, all = true).map { it.id }) }
            var hidden by remember { mutableStateOf(prefs.hiddenLibraries) }

            fun move(
                id: String,
                by: Int,
            ) {
                val i = order.indexOf(id)
                val j = i + by
                if (i < 0 || j !in order.indices) return
                order = order.toMutableList().apply { add(j, removeAt(i)) }
                prefs.libraryOrder = order
            }
            order.forEachIndexed { n, id ->
                val lib = libs.firstOrNull { it.id == id } ?: return@forEachIndexed
                if (n > 0) Divider()
                val shown = id !in hidden
                ListRow(
                    lib.title,
                    subtitle = if (shown) tr("Shown") else tr("Hidden"),
                    icon = if (lib.kind == LibraryKind.SHOWS) Icons.Tv else Icons.Movie,
                    onClick = {
                        hidden = if (shown) hidden + id else hidden - id
                        prefs.hiddenLibraries = hidden
                    },
                    trailing = {
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            if (n > 0) IconAction(Icons.ChevronUp, tr("Move up"), { move(id, -1) })
                            if (n < order.lastIndex) IconAction(Icons.ChevronDown, tr("Move down"), { move(id, 1) })
                        }
                    },
                )
            }
        }
    }
    Group(tr("Pages and app")) {
        var theme by remember { mutableStateOf(settings.themeMusic) }
        ToggleRow(
            tr("Theme music on show pages"),
            theme,
            {
                theme = it
                settings.themeMusic = it
            },
            subtitle = tr("Quietly, when the server has the show's theme."),
        )
        if (d.isTv) {
            Divider()
            var clock by remember { mutableStateOf(prefs.clock) }
            ToggleRow(tr("Clock"), clock, {
                clock = it
                prefs.clock = it
            }, subtitle = tr("In the corner of every page."))
        }
        Divider()
        var confirm by remember { mutableStateOf(prefs.confirmQuit) }
        ToggleRow(tr("Confirm before quitting"), confirm, {
            confirm = it
            prefs.confirmQuit = it
        }, subtitle = tr("Back on the home asks first."))
    }
}

@Composable
private fun DownloadsSection() {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    Group(tr("Delete downloads"), footer = tr("For new downloads; a show's \"next episodes\" keeps its own choice.")) {
        var rule by remember { mutableStateOf(core.defaultDeleteRule()) }
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text(tr("When to delete"), style = androidx.compose.material3.MaterialTheme.typography.titleSmall)
            RuleChips(rule) {
                rule = it
                core.setDefaultDeleteRule(it)
            }
        }
        Divider()
        ListRow(tr("Manage downloads"), icon = Icons.Download, onClick = { nav.push(Route.ManageDownloads) }, trailing = { Chevron() })
    }
}

@Composable
private fun AboutSection() {
    val core = LocalCore.current
    val ctx = LocalContext.current
    Group(
        tr("Language"),
    ) {
        val system =
            tr("Same as the device")
        val options =
            listOf("" to system) +
                I18n
                    .languages()
        ChoiceRow(
            tr("Language"),
            options,
            I18n
                .choice(ctx),
            {
                I18n
                    .choose(ctx, it)
                // Every screen again, in the new language (the place in the app is kept).
                (ctx as? android.app.Activity)?.recreate()
            },
        )
    }
    var cacheMb by remember { mutableStateOf(core.imageCacheBytes().toLong() / (1024 * 1024)) }
    Group(tr("Storage"), footer = tr("Posters and backdrops kept to show pages instantly; they come back when needed.")) {
        ListRow(tr("Image cache"), icon = Icons.Library, trailing = {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                Value(tr("{size} MB", "size" to cacheMb))
                ActionButton(tr("Clear"), {
                    core.clearImageCache()
                    Images.clearMemory()
                    cacheMb = 0
                })
            }
        })
    }
    Group(tr("About"), footer = tr("Free software under the GNU AGPL v3 or later.")) {
        ListRow("AlwyWatch ${BuildConfig.VERSION_NAME}", subtitle = tr("Core {version}", "version" to coreVersion()), icon = Icons.Info)
    }
}
