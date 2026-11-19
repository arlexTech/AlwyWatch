// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.SignedIn
import be.alwyapps.watch.core.keep.ServerEntry
import be.alwyapps.watch.core.keep.ServerType
import be.alwyapps.watch.core.media.PlexServerResource
import be.alwyapps.watch.core.media.QuickConnectCode
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.IconAction
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Outline
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.SurfaceHigh
import be.alwyapps.watch.ui.ambientGlow
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.initialFocus
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

private sealed interface Step {
    data object Choose : Step

    data object JellyfinUrl : Step

    data class Jellyfin(
        val url: String,
        val quick: QuickConnectCode?,
    ) : Step

    data object Plex : Step

    data class PlexServers(
        val accountToken: String,
        val servers: List<PlexServerResource>,
    ) : Step
}

/** Adds a Jellyfin or Plex account. Calls [onSignedIn] once the token is safely stored. */
@Composable
fun AddServerScreen(onSignedIn: (accountId: String) -> Unit) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    var step by remember { mutableStateOf<Step>(Step.Choose) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    // Picked from an AlwyWatch server's list: Plex then selects that server by itself.
    var presetPlexId by remember { mutableStateOf<String?>(null) }
    androidx.activity.compose.ReportDrawn()

    fun finish(s: SignedIn) {
        ctx.services.tokens.put(s.account.id, s.token)
        // Plex: the plex.tv account token is what Seerr and the keep service accept.
        s.plexAccountToken?.let { ctx.services.tokens.put("${s.account.id}:plextv", it) }
        core.setSetting("current_account", s.account.id)
        // Leaving soon + Seerr through the AlwyWatch server the user started from (if any).
        val keepToken = s.plexAccountToken ?: s.token
        ctx.services.scope.launch { runCatching { core.keepOnboard(s.account.id, keepToken) } }
        onSignedIn(s.account.id)
    }

    fun attempt(block: suspend () -> Unit) {
        if (busy) return
        busy = true
        error = null
        scope.launch {
            try {
                block()
            } catch (e: CoreException) {
                error = e.message
            } finally {
                busy = false
            }
        }
    }

    Column(
        Modifier
            .fillMaxSize()
            .ambientGlow(Accent.copy(alpha = 0.55f))
            .safeDrawingPadding()
            .imePadding()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = d.gutter, vertical = 24.dp)
            .widthIn(max = 600.dp),
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        if (step == Step.Choose) {
            Column(Modifier.padding(top = 24.dp, bottom = 8.dp)) {
                Text("AlwyWatch", style = MaterialTheme.typography.displayLarge)
                Text("Your Jellyfin and Plex, fast and beautiful.", style = MaterialTheme.typography.bodyLarge, color = Muted)
            }
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                IconAction(Icons.Back, "Back", {
                    step = Step.Choose
                    error = null
                })
                Text(
                    when (step) {
                        Step.Plex, is Step.PlexServers -> "Plex"
                        else -> "Jellyfin"
                    },
                    style = MaterialTheme.typography.headlineMedium,
                )
            }
        }
        when (val s = step) {
            Step.Choose -> {
                Text("Sign in with your existing account", style = MaterialTheme.typography.titleMedium)
                ChoiceCard("Jellyfin", "Quick Connect or password", Icons.Library, { step = Step.JellyfinUrl }, Modifier.initialFocus())
                ChoiceCard("Plex", "Sign in on plex.tv", Icons.Play, { step = Step.Plex })
                AlwyWatchServerPicker(
                    busy = busy,
                    onPick = { url, entry ->
                        attempt {
                            core.keepSetPreset(url)
                            when (entry.kind) {
                                ServerType.JELLYFIN -> {
                                    core.jellyfinServerInfo(entry.url)
                                    val quick = runCatching { core.jellyfinQuickConnectStart(entry.url) }.getOrNull()
                                    step = Step.Jellyfin(entry.url, quick)
                                }
                                ServerType.PLEX -> {
                                    presetPlexId = entry.id
                                    step = Step.Plex
                                }
                            }
                        }
                    },
                    onError = { error = it },
                )
            }

            Step.JellyfinUrl -> {
                var url by remember { mutableStateOf("") }
                val submit = {
                    attempt {
                        val normalized =
                            be.alwyapps.watch.core
                                .normalizeServerUrl(url)
                        core.jellyfinServerInfo(normalized)
                        val quick = runCatching { core.jellyfinQuickConnectStart(normalized) }.getOrNull()
                        step = Step.Jellyfin(normalized, quick)
                    }
                }
                Field(
                    url,
                    { url = it },
                    label = { Text("Server address") },
                    placeholder = { Text("jellyfin.example.org") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { submit() }),
                    modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
                )
                Text("Your Jellyfin server's address, as you use it in a browser.", color = Muted, style = MaterialTheme.typography.bodyMedium)
                ActionButton(if (busy) "Checking…" else "Continue", { submit() }, primary = true)
            }

            is Step.Jellyfin -> {
                if (s.quick != null) {
                    Column(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(24.dp))
                            .background(SurfaceColor)
                            .border(1.dp, Outline, RoundedCornerShape(24.dp))
                            .padding(24.dp),
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        Text("QUICK CONNECT", style = MaterialTheme.typography.labelSmall, color = Accent)
                        Text(
                            s.quick.code
                                .chunked(3)
                                .joinToString(" "),
                            style =
                                MaterialTheme.typography.displayLarge.copy(
                                    letterSpacing =
                                        androidx.compose.ui.unit
                                            .TextUnit(0.12f, androidx.compose.ui.unit.TextUnitType.Em),
                                ),
                        )
                        Text(
                            "In a Jellyfin app where you're signed in, open your profile → Quick Connect and enter this code.",
                            color = Muted,
                            style = MaterialTheme.typography.bodyMedium,
                        )
                        Text("Waiting for approval…", style = MaterialTheme.typography.bodySmall, color = Subtle)
                    }
                    LaunchedEffect(s.quick.secret) {
                        while (true) {
                            delay(3000)
                            try {
                                finish(core.jellyfinQuickConnectPoll(s.url, s.quick.secret))
                                break
                            } catch (_: CoreException.Pending) {
                            } catch (e: CoreException) {
                                error = e.message
                                break
                            }
                        }
                    }
                    Text("Or with your password", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
                }
                var user by remember { mutableStateOf("") }
                var pass by remember { mutableStateOf("") }
                Field(
                    user,
                    { user = it },
                    label = { Text("Username") },
                    singleLine = true,
                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                )
                Field(
                    pass,
                    { pass = it },
                    label = { Text("Password") },
                    singleLine = true,
                    visualTransformation = PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, imeAction = ImeAction.Done),
                    keyboardActions = KeyboardActions(onDone = { attempt { finish(core.jellyfinSignIn(s.url, user, pass)) } }),
                    modifier = Modifier.fillMaxWidth().dpadEscape(),
                )
                ActionButton(if (busy) "Signing in…" else "Sign in", {
                    attempt { finish(core.jellyfinSignIn(s.url, user, pass)) }
                }, primary = true)
            }

            Step.Plex -> {
                // TV: short code for plex.tv/link. Phone: open the Plex login page.
                val strong = !d.isTv
                var code by remember { mutableStateOf<String?>(null) }
                LaunchedEffect(Unit) {
                    try {
                        val pin = core.plexCreatePin(strong)
                        code = pin.code
                        if (strong && pin.authUrl.isNotEmpty()) {
                            runCatching { ctx.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(pin.authUrl))) }
                        }
                        while (true) {
                            delay(2000)
                            try {
                                val token = core.plexCheckPin(pin.id)
                                val servers = core.plexServers(token)
                                val preset = servers.firstOrNull { it.machineId == presetPlexId }
                                if (preset != null) {
                                    finish(core.plexConnect(token, preset, null))
                                } else {
                                    step = Step.PlexServers(token, servers)
                                }
                                break
                            } catch (_: CoreException.Pending) {
                            }
                        }
                    } catch (e: CoreException) {
                        error = e.message
                    }
                }
                Column(
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(24.dp))
                        .background(SurfaceColor)
                        .border(1.dp, Outline, RoundedCornerShape(24.dp))
                        .padding(24.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    if (strong) {
                        Text("Finish signing in to Plex in your browser, then come back here.", style = MaterialTheme.typography.titleMedium)
                    } else {
                        Text("PLEX.TV/LINK", style = MaterialTheme.typography.labelSmall, color = Accent)
                        Text(code ?: "…", style = MaterialTheme.typography.displayLarge)
                        Text("On your phone or computer, go to plex.tv/link and enter this code.", color = Muted)
                    }
                    Text("Waiting for Plex…", style = MaterialTheme.typography.bodySmall, color = Subtle)
                }
            }

            is Step.PlexServers -> {
                Text("Choose a server", style = MaterialTheme.typography.titleMedium)
                if (s.servers.isEmpty()) Text("This Plex account has no servers.", color = Muted)
                s.servers.forEachIndexed { i, srv ->
                    ChoiceCard(
                        srv.name,
                        if (srv.owned) "Your server" else "Shared with you",
                        Icons.Library,
                        { attempt { finish(core.plexConnect(s.accountToken, srv, null)) } },
                        if (i == 0) Modifier.initialFocus() else Modifier,
                    )
                }
            }
        }
        error?.let { Text(it, color = Danger) }
    }
}

/** "Got an AlwyWatch server address?": lists its servers so nobody types media server URLs. */
@Composable
private fun AlwyWatchServerPicker(
    busy: Boolean,
    onPick: (url: String, entry: ServerEntry) -> Unit,
    onError: (String?) -> Unit,
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var url by remember { mutableStateOf("") }
    var entries by remember { mutableStateOf<Pair<String, List<ServerEntry>>?>(null) }
    val load = {
        onError(null)
        scope.launch {
            try {
                val normalized =
                    be.alwyapps.watch.core
                        .normalizeServerUrl(url)
                entries = normalized to core.keepServers(normalized)
            } catch (e: CoreException) {
                onError(e.message)
            }
        }
    }
    Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.weight(1f).height(1.dp).background(Outline))
        Text("or", color = Subtle, style = MaterialTheme.typography.bodySmall)
        Box(Modifier.weight(1f).height(1.dp).background(Outline))
    }
    Text("Got an AlwyWatch server address from your server's owner?", color = Muted, style = MaterialTheme.typography.bodyMedium)
    Field(
        url,
        { url = it },
        placeholder = { Text("watch.example.org") },
        pill = true,
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
        keyboardActions = KeyboardActions(onGo = { load() }),
        modifier = Modifier.fillMaxWidth().dpadEscape(),
    )
    ActionButton("Show servers", { load() })
    entries?.let { (base, list) ->
        if (list.isEmpty()) Text("This AlwyWatch server lists no media servers.", color = Muted)
        for (e in list) {
            val kind = if (e.kind == ServerType.PLEX) "Plex" else "Jellyfin"
            ActionButton("${e.name} ($kind)", { if (!busy) onPick(base, e) }, primary = true)
        }
    }
}

/** Big tappable card for the sign-in choices. */
@Composable
private fun ChoiceCard(
    title: String,
    subtitle: String,
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    FocusCard(onClick = onClick, modifier = modifier.fillMaxWidth(), shape = RoundedCornerShape(20.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(SurfaceColor)
                .border(1.dp, Outline, RoundedCornerShape(20.dp))
                .padding(18.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Box(
                Modifier
                    .size(48.dp)
                    .clip(RoundedCornerShape(14.dp))
                    .background(SurfaceHigh),
                contentAlignment = Alignment.Center,
            ) { androidx.compose.material3.Icon(icon, null, Modifier.size(26.dp), tint = Accent) }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium)
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = Muted)
            }
            Chevron()
        }
    }
}
