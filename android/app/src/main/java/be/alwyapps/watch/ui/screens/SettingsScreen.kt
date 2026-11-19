// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
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
import be.alwyapps.watch.ui.dpadEscape
import kotlinx.coroutines.launch

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

        Group("Accounts") {
            accounts.forEachIndexed { i, a ->
                if (i > 0) Divider()
                ListRow(
                    a.userName,
                    subtitle = "${a.serverName} · ${if (a.kind == be.alwyapps.watch.core.media.ServerKind.PLEX) "Plex" else "Jellyfin"}",
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

        val keep = remember(version) { core.keepConnection() }
        Group("Leaving soon", footer = "Your AlwyWatch server shows what will be deleted and lets you keep it.") {
            if (keep == null) {
                var keepUrl by remember { mutableStateOf("") }
                var keepError by remember { mutableStateOf<String?>(null) }
                val scope = androidx.compose.runtime.rememberCoroutineScope()
                Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    be.alwyapps.watch.ui.Field(
                        keepUrl,
                        { keepUrl = it },
                        Modifier.fillMaxWidth().dpadEscape(),
                        label = { Text("AlwyWatch server address") },
                        singleLine = true,
                    )
                    ActionButton("Connect as ${accounts.firstOrNull { it.id == current }?.userName ?: "this account"}", {
                        val account = accounts.firstOrNull { it.id == current } ?: return@ActionButton
                        val token =
                            if (account.kind == be.alwyapps.watch.core.media.ServerKind.PLEX) {
                                ctx.services.tokens.get("${account.id}:plextv")
                            } else {
                                ctx.services.tokens.get(account.id)
                            }
                        if (token == null) {
                            keepError = "Sign in to this account again first."
                            return@ActionButton
                        }
                        scope.launch {
                            try {
                                core.keepConnect(
                                    be.alwyapps.watch.core
                                        .normalizeServerUrl(keepUrl),
                                    account.id,
                                    token,
                                )
                                keepError = null
                                // Seerr through the same server when not set up yet (optional there).
                                if (core.seerrConnection() == null) runCatching { core.seerrConnectBridge() }
                                version++
                            } catch (e: be.alwyapps.watch.core.CoreException) {
                                keepError = e.message
                            }
                        }
                    }, primary = true)
                    keepError?.let { Text(it, color = be.alwyapps.watch.ui.Danger) }
                }
            } else {
                ListRow(
                    keep.userName + if (keep.isAdmin) " · admin" else "",
                    subtitle = keep.baseUrl,
                    icon = Icons.Hourglass,
                    onClick = { nav.push(Route.LeavingSoon) },
                    trailing = { Chevron() },
                )
                if (keep.isAdmin) {
                    Divider()
                    ListRow(
                        "Admin",
                        subtitle = "Rights and countdown",
                        icon = Icons.Settings,
                        onClick = { nav.push(Route.KeepAdmin) },
                        trailing = { Chevron() },
                    )
                }
                Divider()
                ListRow("Disconnect", icon = Icons.Close, iconTint = be.alwyapps.watch.ui.Danger, onClick = {
                    core.keepDisconnect()
                    version++
                })
            }
        }

        val seerr = remember(version) { core.seerrConnection() }
        Group("Requests") {
            if (seerr == null) {
                ListRow(
                    "Connect Seerr",
                    subtitle = "Ask for movies and shows",
                    icon = Icons.Add,
                    onClick = { nav.push(Route.SeerrConnect) },
                    trailing = { Chevron() },
                )
            } else {
                ListRow(
                    seerr.userName,
                    subtitle = if (seerr.viaKeep) "Seerr, through your AlwyWatch server" else "Seerr · ${seerr.baseUrl}",
                    icon = Icons.Add,
                    onClick = { nav.push(Route.Requests) },
                    trailing = { Chevron() },
                )
                Divider()
                ListRow("Disconnect", icon = Icons.Close, iconTint = be.alwyapps.watch.ui.Danger, onClick = {
                    core.seerrSignOut()
                    ctx.services.tokens.remove("seerr")
                    version++
                })
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
