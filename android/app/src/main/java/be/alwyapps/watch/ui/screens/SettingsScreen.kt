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
import be.alwyapps.watch.ui.Accent
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
import be.alwyapps.watch.ui.OnSurface
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
            accounts.forEachIndexed { i, a ->
                if (i > 0) Divider()
                val sync =
                    when {
                        keep?.accountId == a.id -> "Synced with your AlwyWatch server"
                        keep != null && served?.contains(a.serverId) == true -> "On your AlwyWatch server · not synced"
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
                    icon = Icons.Person,
                    iconTint = if (a.id == current) Accent else OnSurface,
                    onClick =
                        if (a.id != current) {
                            {
                                core.setSetting("current_account", a.id)
                                onAccountChanged(a.id)
                            }
                        } else {
                            null
                        },
                    trailing = {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            if (a.id == current) Value("In use")
                            IconAction(Icons.Delete, "Remove ${a.userName}", {
                                core.removeAccount(a.id)
                                ctx.services.tokens.remove(a.id)
                                version++
                                if (a.id == current) onAccountChanged(core.accounts().firstOrNull()?.id)
                            })
                        }
                    },
                )
            }
            Divider()
            ListRow("Add a server", icon = Icons.Add, onClick = { nav.push(Route.AddServer) }, trailing = { Chevron() })
        }

        AlwyWatchServerGroup(accounts, version) { version++ }
        RequestsGroup(version) { version++ }

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
