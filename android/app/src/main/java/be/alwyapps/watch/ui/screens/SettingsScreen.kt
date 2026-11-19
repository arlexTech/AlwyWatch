// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.BuildConfig
import be.alwyapps.watch.core.coreVersion
import be.alwyapps.watch.player.PlayerSettings
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Images
import be.alwyapps.watch.ui.LocalAccount
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalNavigator
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Route
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
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(d.gutter),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Text("Settings", style = MaterialTheme.typography.titleLarge)

        Text("Accounts", style = MaterialTheme.typography.titleMedium)
        for (a in accounts) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Column(Modifier.weight(1f)) {
                    Text("${a.userName} @ ${a.serverName}")
                    Text("${a.kind.name.lowercase()} · ${a.baseUrl}", color = Muted, style = MaterialTheme.typography.bodySmall)
                }
                if (a.id != current) {
                    ActionButton("Use", {
                        core.setSetting("current_account", a.id)
                        onAccountChanged(a.id)
                    })
                } else {
                    Text("In use", color = Muted)
                }
                ActionButton("Remove", {
                    core.removeAccount(a.id)
                    ctx.services.tokens.remove(a.id)
                    version++
                    if (a.id == current) onAccountChanged(core.accounts().firstOrNull()?.id)
                })
            }
        }
        ActionButton("Add a server", { nav.push(Route.AddServer) }, primary = true)

        Text("Leaving soon (AlwyWatch server)", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 12.dp))
        val keep = remember(version) { core.keepConnection() }
        if (keep == null) {
            var keepUrl by remember { mutableStateOf("") }
            var keepError by remember { mutableStateOf<String?>(null) }
            val scope = androidx.compose.runtime.rememberCoroutineScope()
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
            })
            keepError?.let { Text(it, color = be.alwyapps.watch.ui.Danger) }
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("${keep.userName} @ ${keep.baseUrl}" + if (keep.isAdmin) " (admin)" else "", Modifier.weight(1f))
                ActionButton("Leaving soon", { nav.push(Route.LeavingSoon) })
                ActionButton("Disconnect", {
                    core.keepDisconnect()
                    version++
                })
            }
        }

        Text("Requests (Seerr)", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 12.dp))
        val seerr = remember(version) { core.seerrConnection() }
        if (seerr == null) {
            ActionButton("Connect Seerr", { nav.push(Route.SeerrConnect) })
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(
                    if (seerr.viaKeep) "${seerr.userName} (through your AlwyWatch server)" else "${seerr.userName} @ ${seerr.baseUrl}",
                    Modifier.weight(1f),
                )
                ActionButton("Disconnect", {
                    core.seerrSignOut()
                    ctx.services.tokens.remove("seerr")
                    version++
                })
            }
        }

        Text("Playback", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 12.dp))
        var tunneling by remember { mutableStateOf(settings.tunneling) }
        ActionButton("Tunneled playback: ${if (tunneling) "on" else "off"}", {
            tunneling = !tunneling
            settings.tunneling = tunneling
        })
        Text(
            "Lower power on some TV devices, but breaks playback on others. Off by default.",
            color = Muted,
            style = MaterialTheme.typography.bodySmall,
        )
        var matchRate by remember { mutableStateOf(settings.matchFrameRate) }
        ActionButton("Match display refresh rate: ${if (matchRate) "on" else "off"}", {
            matchRate = !matchRate
            settings.matchFrameRate = matchRate
        })

        Text("Storage", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 12.dp))
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Text("Image cache: $cacheMb MB", Modifier.weight(1f))
            ActionButton("Clear", {
                core.clearImageCache()
                Images.clearMemory()
                cacheMb = 0
            })
        }

        Text("About", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 12.dp))
        Text("AlwyWatch ${BuildConfig.VERSION_NAME} (core ${coreVersion()})", color = Muted)
        Text("Free software under the GNU AGPL v3 or later.", color = Muted)
    }
}
