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
import be.alwyapps.watch.ui.usesRail

/** A page of Settings, opened from the list (`Route.SettingsSection`). */
private enum class Section(
    val id: String,
    val title: String,
    val icon: ImageVector,
) {
    SERVERS("servers", "Servers & accounts", Icons.Library),
    SERVICES("services", "AlwyWatch server & requests", Icons.Hourglass),
    PROFILES("profiles", "Profiles", Icons.Person),
    PLAYBACK("playback", "Playback", Icons.Play),
    AUDIO("audio", "Audio & subtitles", Icons.Subtitles),
    INTERFACE("interface", "Menu & pages", Icons.Home),
    DOWNLOADS("downloads", "Downloads", Icons.Download),
    ABOUT("about", "About & storage", Icons.Info),
    ;

    companion object {
        fun of(id: String) = entries.firstOrNull { it.id == id } ?: ABOUT
    }
}

private fun kindName(kind: ServerKind) = if (kind == ServerKind.PLEX) "Plex" else "Jellyfin"

/** The scrolling column every Settings page uses. */
@Composable
private fun SettingsPage(
    title: String,
    content: @Composable () -> Unit,
) {
    val d = LocalDimens.current
    // Back on this page: the row that was opened gets the focus (else its first row, TV).
    // TV: rows scroll into view just enough (the default keeps the focused row a third of the
    // way down: a page opening on a row under its first group had its title half off screen).
    androidx.compose.runtime.CompositionLocalProvider(be.alwyapps.watch.ui.LocalRowMemory provides "settings.$title") {
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
    val settings = remember { PlayerSettings(core) }
    val accounts = remember { core.accounts() }
    val keep = remember { core.keepConnection() }
    val seerr = remember { core.seerrConnection() }
    val rail = usesRail()

    fun on(b: Boolean) = if (b) "on" else "off"

    fun summary(s: Section): String =
        when (s) {
            Section.SERVERS ->
                accounts.joinToString(" · ") { "${it.serverName} (${kindName(it.kind)})" }.ifEmpty { "No server yet" }
            Section.SERVICES ->
                listOf(
                    keep?.let { "AlwyWatch server ${it.baseUrl.substringAfter("://").trimEnd('/')}" } ?: "No AlwyWatch server",
                    if (seerr != null) "requests on" else "requests off",
                ).joinToString(" · ")
            Section.PROFILES -> "Who's watching on this device, PINs"
            Section.PLAYBACK ->
                "Skip intros ${on(settings.autoSkipIntro)} · next episode ${on(settings.autoPlayNext)} · " +
                    "−${settings.seekBackS} s / +${settings.seekForwardS} s"
            Section.AUDIO ->
                listOf(
                    languageName(settings.audioLanguage.ifEmpty { null }) ?: "Original audio",
                    "subtitles " +
                        when (settings.subtitleMode) {
                            "forced" -> "forced only"
                            "always" -> "always"
                            "off" -> "off"
                            else -> "automatic"
                        },
                    settings.subtitleSize,
                ).joinToString(" · ")
            Section.INTERFACE ->
                listOfNotNull(
                    "libraries in the menu".takeIf { rail },
                    "clock".takeIf { d.isTv },
                    "theme music",
                    "quitting",
                ).joinToString(", ").replaceFirstChar { it.uppercase() }
            Section.DOWNLOADS -> "When to delete, manage downloads"
            Section.ABOUT -> "AlwyWatch ${BuildConfig.VERSION_NAME} · image cache"
        }

    SettingsPage("Settings") {
        Group(null) {
            Section.entries
                .filter { it != Section.DOWNLOADS || !d.isTv }
                .forEachIndexed { i, s ->
                    if (i > 0) Divider()
                    ListRow(
                        s.title,
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
    SettingsPage(s.title) {
        when (s) {
            Section.SERVERS -> ServersSection(onAccountChanged)
            Section.SERVICES -> {
                val core = LocalCore.current
                var version by remember { mutableIntStateOf(0) }
                val accounts = remember(version) { core.accounts() }
                if (remember(version) { core.keepConnection() } != null) AlwyWatchAccountGroup()
                AlwyWatchServerGroup(accounts, version) { version++ }
                RequestsGroup(version) { version++ }
            }
            Section.PROFILES -> ProfilesGroup()
            Section.PLAYBACK -> PlaybackSection()
            Section.AUDIO -> AudioSection()
            Section.INTERFACE -> InterfaceSection()
            Section.DOWNLOADS -> DownloadsSection()
            Section.ABOUT -> AboutSection()
        }
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
    // Which accounts sync with the AlwyWatch server (and which servers it serves).
    val keep = remember(version) { core.keepConnection() }
    var served by remember(version) { mutableStateOf<Set<String>?>(null) }
    androidx.compose.runtime.LaunchedEffect(keep?.baseUrl) {
        val url = keep?.baseUrl ?: return@LaunchedEffect
        served = runCatching { core.keepServers(url).map { it.id }.toSet() }.getOrNull()
    }
    var removing by remember { mutableStateOf<be.alwyapps.watch.core.Account?>(null) }
    removing?.let { a ->
        be.alwyapps.watch.ui.ConfirmDialog(
            "Remove ${a.userName}?",
            "You'll be signed out of ${a.serverName} on this device. Its downloads stay until you delete them.",
            confirm = "Remove",
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
        "Your servers",
        footer =
            if (keep != null) {
                "One account syncs with your AlwyWatch server at a time (leaving soon, keep choices, requests). " +
                    "Change it in AlwyWatch server & requests."
            } else {
                null
            },
    ) {
        // Together: the preferred account is also the one in use (search, leaving soon…).
        val main = if (together && accounts.size > 1) core.getSetting(PREFERRED_KEY)?.takeIf { p -> accounts.any { it.id == p } } ?: current else current
        accounts.forEachIndexed { i, a ->
            if (i > 0) Divider()
            val sync =
                when {
                    keep?.accountId == a.id -> "Synced with AlwyWatch"
                    keep != null && served?.contains(a.serverId) == true -> "On AlwyWatch · not synced"
                    keep != null && served == null -> null
                    else -> "Only on this device"
                }
            ListRow(
                a.userName,
                subtitle = listOfNotNull("${a.serverName} · ${kindName(a.kind)}", sync).joinToString("\n"),
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
                        if (a.id == main) Value(if (together && accounts.size > 1) "Preferred" else "In use")
                        if (a.kind == ServerKind.PLEX) {
                            IconAction(Icons.Person, "Switch Plex user", { nav.push(Route.PlexSwitch(a.id)) })
                        }
                        IconAction(Icons.Delete, "Remove ${a.userName}", { removing = a })
                    }
                },
            )
        }
        Divider()
        ListRow("Add a server", icon = Icons.Add, onClick = { nav.push(Route.AddServer) }, trailing = { Chevron() })
    }
    if (accounts.size > 1) {
        Group("Home", footer = "Together, the home mixes every server; a title on several is shown once, from the preferred one.") {
            ToggleRow(
                "Show all servers together",
                together,
                {
                    together = it
                    core.setSetting(COMBINED_KEY, if (it) "1" else "0")
                },
                subtitle =
                    if (together) {
                        "${if (d.isTv) "Select" else "Tap"} an account above to make it the preferred one"
                    } else {
                        "One server at a time: ${if (d.isTv) "select" else "tap"} an account above to switch"
                    },
            )
        }
        SyncGroup(accounts)
        Group("Moving between servers", footer = "A one-time copy, one way. Each person copies their own history, with their own accounts.") {
            ListRow(
                "Copy watch history",
                subtitle = "What you watched and where you stopped, from one server to another (Plex to Jellyfin)",
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
        "Keep in sync",
        footer =
            "Both ways: what you watch, unwatch or where you stop on one server is copied to the other. " +
                "Right away for what you watch in AlwyWatch, every 6 hours for other apps. " +
                "Changed on both since the last sync: the further one wins.",
    ) {
        candidates.forEachIndexed { i, (a, b) ->
            if (i > 0) Divider()
            val on = pairs.any { (it.a == a.id && it.b == b.id) || (it.a == b.id && it.b == a.id) }
            val status = remember(version, on) { runCatching { core.syncStatus(a.id, b.id) }.getOrNull() }
            val subtitle =
                when {
                    !on -> "${a.serverName} (${kindName(a.kind)}) and ${b.serverName} (${kindName(b.kind)})"
                    state.running -> "Syncing now…"
                    status?.error != null -> "Last sync failed: ${status.error}"
                    status?.lastRunAt != null -> {
                        val at =
                            java.text.DateFormat
                                .getTimeInstance(java.text.DateFormat.SHORT)
                                .format(java.util.Date(status.lastRunAt!! * 1000))
                        val changed = status.changes.toInt()
                        "Last synced $at · " + if (changed == 0) "nothing to change" else "$changed updated"
                    }
                    else -> "Waiting for the first sync"
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
                if (state.running) "Syncing…" else "Sync now",
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
    Group("Intros and credits") {
        var autoSkip by remember { mutableStateOf(settings.autoSkipIntro) }
        ToggleRow(
            "Skip intros automatically",
            autoSkip,
            {
                autoSkip = it
                settings.autoSkipIntro = it
            },
            subtitle = "Otherwise a Skip intro button appears.",
        )
        Divider()
        var autoNext by remember { mutableStateOf(settings.autoPlayNext) }
        ToggleRow(
            "Play next episode during credits",
            autoNext,
            {
                autoNext = it
                settings.autoPlayNext = it
            },
            subtitle = "After a countdown, which Back cancels.",
        )
        Divider()
        SecondsRow(
            "Next episode countdown",
            listOf("5" to "5 seconds", "10" to "10 seconds", "15" to "15 seconds", "30" to "30 seconds"),
            { settings.countdownS },
            { settings.countdownS = it },
        )
    }
    Group("Remote and resuming") {
        SecondsRow(
            "Skip back",
            listOf("5" to "5 seconds", "10" to "10 seconds", "15" to "15 seconds", "30" to "30 seconds"),
            { settings.seekBackS },
            { settings.seekBackS = it },
        )
        Divider()
        SecondsRow(
            "Skip forward",
            listOf("10" to "10 seconds", "15" to "15 seconds", "30" to "30 seconds", "60" to "1 minute"),
            { settings.seekForwardS },
            { settings.seekForwardS = it },
        )
        Divider()
        SecondsRow(
            "Rewind when resuming",
            listOf("0" to "Off", "5" to "5 seconds", "10" to "10 seconds", "30" to "30 seconds"),
            { settings.resumeRewindS },
            { settings.resumeRewindS = it },
        )
        Divider()
        SecondsRow(
            "Are you still watching?",
            listOf("0" to "Never", "2" to "After 2 hours", "3" to "After 3 hours", "4" to "After 4 hours"),
            { settings.stillWatchingH },
            { settings.stillWatchingH = it },
            subtitle = "Pauses when the remote has not been touched for that long.",
        )
    }
    Group("Picture") {
        var matchRate by remember { mutableStateOf(settings.matchFrameRate) }
        ToggleRow(
            "Match display refresh rate",
            matchRate,
            {
                matchRate = it
                settings.matchFrameRate = it
            },
            subtitle = "Switches the TV to the video's frame rate (no judder).",
        )
        // Tunneling only exists on TV devices.
        if (d.isTv) {
            Divider()
            var tunneling by remember { mutableStateOf(settings.tunneling) }
            ToggleRow(
                "Tunneled playback",
                tunneling,
                {
                    tunneling = it
                    settings.tunneling = it
                },
                subtitle = "Lower power on some TV devices, but breaks playback on others.",
            )
        }
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
        "Languages",
        footer = "A movie or show remembers the tracks picked for it (on its page or while watching), and its next episodes follow.",
    ) {
        var audio by remember { mutableStateOf(settings.audioLanguage) }
        ChoiceRow("Audio language", listOf("" to "Original (the file's default)") + languages, audio, {
            audio = it
            settings.audioLanguage = it
        })
        Divider()
        var mode by remember { mutableStateOf(settings.subtitleMode) }
        ChoiceRow(
            "Subtitles",
            listOf("auto" to "Automatic", "forced" to "Forced only", "always" to "Always", "off" to "Off"),
            mode,
            {
                mode = it
                settings.subtitleMode = it
            },
            subtitle = "Automatic: when the audio is not in your subtitle language.",
        )
        Divider()
        var subLang by remember { mutableStateOf(settings.subtitleLanguage) }
        val device =
            languageName(
                java.util.Locale
                    .getDefault()
                    .language,
            ) ?: "?"
        ChoiceRow("Subtitle language", listOf("" to "Device language ($device)") + languages, subLang, {
            subLang = it
            settings.subtitleLanguage = it
        })
    }
    Group("Subtitle look") {
        var size by remember { mutableStateOf(settings.subtitleSize) }
        ChoiceRow("Size", listOf("small" to "Small", "normal" to "Normal", "large" to "Large", "huge" to "Huge"), size, {
            size = it
            settings.subtitleSize = it
        })
        Divider()
        var color by remember { mutableStateOf(settings.subtitleColor) }
        ChoiceRow("Colour", listOf("white" to "White", "yellow" to "Yellow"), color, {
            color = it
            settings.subtitleColor = it
        })
        Divider()
        var box by remember { mutableStateOf(settings.subtitleBackground) }
        ToggleRow("Background box", box, {
            box = it
            settings.subtitleBackground = it
        }, subtitle = "Otherwise a dark outline keeps them readable.")
        Divider()
        var raised by remember { mutableStateOf(settings.subtitleRaised) }
        ToggleRow("Raise subtitles", raised, {
            raised = it
            settings.subtitleRaised = it
        }, subtitle = "Higher on the screen (TVs that crop the edges).")
    }
}

@Composable
private fun InterfaceSection() {
    val core = LocalCore.current
    val d = LocalDimens.current
    val settings = remember { PlayerSettings(core) }
    val prefs = remember { UiPrefs(core, d.isTv) }
    if (usesRail()) {
        Group("Libraries in the menu", footer = "${if (d.isTv) "Select" else "Tap"} a library to show or hide it; arrows change the order.") {
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
                    subtitle = if (shown) "Shown" else "Hidden",
                    icon = if (lib.kind == LibraryKind.SHOWS) Icons.Tv else Icons.Movie,
                    onClick = {
                        hidden = if (shown) hidden + id else hidden - id
                        prefs.hiddenLibraries = hidden
                    },
                    trailing = {
                        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            if (n > 0) IconAction(Icons.ChevronUp, "Move up", { move(id, -1) })
                            if (n < order.lastIndex) IconAction(Icons.ChevronDown, "Move down", { move(id, 1) })
                        }
                    },
                )
            }
        }
    }
    Group("Pages and app") {
        var theme by remember { mutableStateOf(settings.themeMusic) }
        ToggleRow(
            "Theme music on show pages",
            theme,
            {
                theme = it
                settings.themeMusic = it
            },
            subtitle = "Quietly, when the server has the show's theme.",
        )
        if (d.isTv) {
            Divider()
            var clock by remember { mutableStateOf(prefs.clock) }
            ToggleRow("Clock", clock, {
                clock = it
                prefs.clock = it
            }, subtitle = "In the corner of every page.")
        }
        Divider()
        var confirm by remember { mutableStateOf(prefs.confirmQuit) }
        ToggleRow("Confirm before quitting", confirm, {
            confirm = it
            prefs.confirmQuit = it
        }, subtitle = "Back on the home asks first.")
    }
}

@Composable
private fun DownloadsSection() {
    val core = LocalCore.current
    val nav = LocalNavigator.current
    Group("Delete downloads", footer = "For new downloads; a show's \"next episodes\" keeps its own choice.") {
        var rule by remember { mutableStateOf(core.defaultDeleteRule()) }
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("When to delete", style = androidx.compose.material3.MaterialTheme.typography.titleSmall)
            RuleChips(rule) {
                rule = it
                core.setDefaultDeleteRule(it)
            }
        }
        Divider()
        ListRow("Manage downloads", icon = Icons.Download, onClick = { nav.push(Route.ManageDownloads) }, trailing = { Chevron() })
    }
}

@Composable
private fun AboutSection() {
    val core = LocalCore.current
    var cacheMb by remember { mutableStateOf(core.imageCacheBytes().toLong() / (1024 * 1024)) }
    Group("Storage", footer = "Posters and backdrops kept to show pages instantly; they come back when needed.") {
        ListRow("Image cache", icon = Icons.Library, trailing = {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                Value("$cacheMb MB")
                ActionButton("Clear", {
                    core.clearImageCache()
                    Images.clearMemory()
                    cacheMb = 0
                })
            }
        })
    }
    Group("About", footer = "Free software under the GNU AGPL v3 or later.") {
        ListRow("AlwyWatch ${BuildConfig.VERSION_NAME}", subtitle = "Core ${coreVersion()}", icon = Icons.Info)
    }
}
