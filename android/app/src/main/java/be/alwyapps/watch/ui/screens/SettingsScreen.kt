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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.BuildConfig
import be.alwyapps.watch.core.coreVersion
import be.alwyapps.watch.player.PlayerSettings
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Chevron
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
import be.alwyapps.watch.ui.Value

@Composable
fun SettingsScreen(onAccountChanged: (String?) -> Unit) {
    val core = LocalCore.current
    val current = LocalAccount.current
    val nav = LocalNavigator.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    var version by remember { mutableIntStateOf(0) }
    val accounts = remember(version) { core.accounts() }
    var cacheMb by remember(version) { mutableStateOf(core.imageCacheBytes().toLong() / (1024 * 1024)) }
    val settings = remember { PlayerSettings(core) }

    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 16.dp)
            .widthIn(max = 760.dp),
        verticalArrangement = Arrangement.spacedBy(26.dp),
    ) {
        PageTitle("Settings")

        // Which accounts sync with the AlwyWatch server (and which servers it serves).
        val keep = remember(version) { core.keepConnection() }
        var served by remember(version) { mutableStateOf<Set<String>?>(null) }
        androidx.compose.runtime.LaunchedEffect(keep?.baseUrl) {
            val url = keep?.baseUrl ?: return@LaunchedEffect
            served = runCatching { core.keepServers(url).map { it.id }.toSet() }.getOrNull()
        }
        // Several servers: shown together (one preferred for titles on both) or one at a time.
        var together by remember { mutableStateOf(core.getSetting(COMBINED_KEY) != "0") }
        if (accounts.size > 1) {
            Group(null, footer = "Together, the home mixes every server; a title on several is shown once, from the preferred one.") {
                ToggleRow(
                    "Show all servers together",
                    together,
                    {
                        together = it
                        core.setSetting(COMBINED_KEY, if (it) "1" else "0")
                    },
                    subtitle = if (together) "Tap an account to make it the preferred one" else "One server at a time: tap an account to switch",
                )
            }
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
                    version++
                    if (a.id == current) onAccountChanged(core.accounts().firstOrNull()?.id)
                },
                onDismiss = { removing = null },
            )
        }
        Group(
            "Accounts",
            footer =
                if (keep != null) {
                    "One account syncs with your AlwyWatch server at a time (leaving soon, keep choices, requests). " +
                        "Change it under AlwyWatch server."
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
                    subtitle =
                        listOfNotNull(
                            "${a.serverName} · ${if (a.kind == be.alwyapps.watch.core.media.ServerKind.PLEX) "Plex" else "Jellyfin"}",
                            sync,
                        ).joinToString("\n"),
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
                            if (a.kind == be.alwyapps.watch.core.media.ServerKind.PLEX) {
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

        AlwyWatchServerGroup(accounts, version) { version++ }
        RequestsGroup(version) { version++ }

        ProfilesGroup()

        if (!d.isTv) {
            Group("Downloads", footer = "For new downloads; a show's \"next episodes\" keeps its own choice.") {
                var rule by remember { mutableStateOf(core.defaultDeleteRule()) }
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("Delete downloads", style = androidx.compose.material3.MaterialTheme.typography.titleSmall)
                    RuleChips(rule) {
                        rule = it
                        core.setDefaultDeleteRule(it)
                    }
                }
                Divider()
                ListRow("Manage downloads", icon = Icons.Download, onClick = { nav.push(Route.ManageDownloads) }, trailing = { Chevron() })
            }
        }

        Group("Playback") {
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
            Divider()
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
                subtitle = "After a 10 second countdown, which Back cancels.",
            )
            Divider()
            var countdown by remember { mutableStateOf(settings.countdownS.toString()) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Next episode countdown",
                listOf("5" to "5 seconds", "10" to "10 seconds", "15" to "15 seconds", "30" to "30 seconds"),
                countdown,
                {
                    countdown = it
                    settings.countdownS = it.toInt()
                },
            )
            Divider()
            var rewind by remember { mutableStateOf(settings.resumeRewindS.toString()) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Rewind when resuming",
                listOf("0" to "Off", "5" to "5 seconds", "10" to "10 seconds", "30" to "30 seconds"),
                rewind,
                {
                    rewind = it
                    settings.resumeRewindS = it.toInt()
                },
            )
            Divider()
            var back by remember { mutableStateOf(settings.seekBackS.toString()) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Skip back",
                listOf("5" to "5 seconds", "10" to "10 seconds", "15" to "15 seconds", "30" to "30 seconds"),
                back,
                {
                    back = it
                    settings.seekBackS = it.toInt()
                },
            )
            Divider()
            var forward by remember { mutableStateOf(settings.seekForwardS.toString()) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Skip forward",
                listOf("10" to "10 seconds", "15" to "15 seconds", "30" to "30 seconds", "60" to "1 minute"),
                forward,
                {
                    forward = it
                    settings.seekForwardS = it.toInt()
                },
            )
            Divider()
            var still by remember { mutableStateOf(settings.stillWatchingH.toString()) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Are you still watching?",
                listOf("0" to "Never", "2" to "After 2 hours", "3" to "After 3 hours", "4" to "After 4 hours"),
                still,
                {
                    still = it
                    settings.stillWatchingH = it.toInt()
                },
                subtitle = "Pauses when the remote has not been touched for that long.",
            )
            Divider()
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
            var tunneling by remember { mutableStateOf(settings.tunneling) }
            // Tunneling only exists on TV devices.
            if (d.isTv) Divider()
            if (d.isTv) {
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

        Group("Audio & subtitles", footer = "A movie or show remembers the tracks picked for it (on its page or while watching).") {
            val languages =
                listOf(
                    "eng",
                    "fra",
                    "nld",
                    "deu",
                    "spa",
                    "ita",
                    "por",
                    "jpn",
                    "kor",
                    "zho",
                    "ara",
                    "rus",
                    "pol",
                    "swe",
                    "tur",
                    "hin",
                ).map {
                    it to (
                        be.alwyapps.watch.player
                            .languageName(it) ?: it
                    )
                }
            var audio by remember { mutableStateOf(settings.audioLanguage) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Audio language",
                listOf("" to "Original (the file's default)") + languages,
                audio,
                {
                    audio = it
                    settings.audioLanguage = it
                },
            )
            Divider()
            var mode by remember { mutableStateOf(settings.subtitleMode) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Subtitles",
                listOf(
                    "auto" to "Automatic",
                    "forced" to "Forced only",
                    "always" to "Always",
                    "off" to "Off",
                ),
                mode,
                {
                    mode = it
                    settings.subtitleMode = it
                },
                subtitle = "Automatic: when the audio is not in your subtitle language.",
            )
            Divider()
            var subLang by remember { mutableStateOf(settings.subtitleLanguage) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Subtitle language",
                listOf(
                    "" to "Device language (${be.alwyapps.watch.player
                        .languageName(
                            java.util.Locale
                                .getDefault()
                                .language,
                        ) ?: "?"})",
                ) + languages,
                subLang,
                {
                    subLang = it
                    settings.subtitleLanguage = it
                },
            )
            Divider()
            var size by remember { mutableStateOf(settings.subtitleSize) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Subtitle size",
                listOf("small" to "Small", "normal" to "Normal", "large" to "Large", "huge" to "Huge"),
                size,
                {
                    size = it
                    settings.subtitleSize = it
                },
            )
            Divider()
            var color by remember { mutableStateOf(settings.subtitleColor) }
            be.alwyapps.watch.ui.ChoiceRow(
                "Subtitle colour",
                listOf("white" to "White", "yellow" to "Yellow"),
                color,
                {
                    color = it
                    settings.subtitleColor = it
                },
            )
            Divider()
            var box by remember { mutableStateOf(settings.subtitleBackground) }
            ToggleRow("Subtitle background", box, {
                box = it
                settings.subtitleBackground = it
            })
            Divider()
            var raised by remember { mutableStateOf(settings.subtitleRaised) }
            ToggleRow("Raise subtitles", raised, {
                raised = it
                settings.subtitleRaised = it
            }, subtitle = "Higher on the screen (TVs that crop the edges).")
        }

        Group("Storage") {
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
}
