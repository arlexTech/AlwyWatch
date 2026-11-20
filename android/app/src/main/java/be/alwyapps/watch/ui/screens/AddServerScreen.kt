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

    /** "I have an AlwyWatch server": address, then the media servers it serves. */
    data object AlwyWatch : Step

    /** "I don't have one": Jellyfin or Plex directly. */
    data object Direct : Step

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
    // Steps visited, so Back returns to the previous one (AlwyWatch server list, or direct).
    // Already connected to an AlwyWatch server: start from its list (no "how do you watch").
    val keep = remember { core.keepConnection() }
    var history by remember { mutableStateOf(listOf<Step>(if (keep != null) Step.AlwyWatch else Step.Choose)) }
    val step = history.last()

    fun go(next: Step) {
        history = history + next
    }

    fun back() {
        if (history.size > 1) history = history.dropLast(1)
    }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    // Picked from an AlwyWatch server's list: Plex then selects that server by itself.
    var presetPlexId by remember { mutableStateOf<String?>(null) }
    androidx.activity.compose.ReportDrawn()
    androidx.activity.compose.BackHandler(enabled = history.size > 1) {
        back()
        error = null
    }

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
        if (history.size == 1 && step == Step.AlwyWatch) {
            be.alwyapps.watch.ui
                .PageTitle("Add a server", "From your AlwyWatch server, or any other.")
        } else if (step == Step.Choose) {
            Column(Modifier.padding(top = 24.dp, bottom = 8.dp)) {
                Text("AlwyWatch", style = MaterialTheme.typography.displayLarge)
                Text("Your Jellyfin and Plex, fast and beautiful.", style = MaterialTheme.typography.bodyLarge, color = Muted)
            }
        } else {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                IconAction(Icons.Back, "Back", {
                    back()
                    error = null
                })
                Text(
                    when (step) {
                        Step.AlwyWatch -> "AlwyWatch server"
                        Step.Direct -> "Sign in"
                        Step.Plex, is Step.PlexServers -> "Plex"
                        else -> "Jellyfin"
                    },
                    style = MaterialTheme.typography.headlineMedium,
                )
            }
        }
        when (val s = step) {
            Step.Choose -> {
                Text("How do you watch?", style = MaterialTheme.typography.titleMedium)
                ChoiceCard(
                    "I have an AlwyWatch server",
                    "Its address comes from your server's owner. Includes what's leaving soon and requests.",
                    Icons.Hourglass,
                    { go(Step.AlwyWatch) },
                    Modifier.initialFocus(),
                )
                ChoiceCard(
                    "I don't have one",
                    "Sign in to Jellyfin or Plex directly. You can add an AlwyWatch server later in Settings.",
                    Icons.Person,
                    { go(Step.Direct) },
                )
            }

            Step.AlwyWatch ->
                AlwyWatchServerPicker(
                    busy = busy,
                    connectedUrl = keep?.baseUrl,
                    signedIn = remember { core.accounts().map { it.serverId }.toSet() },
                    onOther = { go(Step.Direct) },
                    onPick = { url, entry ->
                        attempt {
                            core.keepSetPreset(url)
                            when (entry.kind) {
                                ServerType.JELLYFIN -> {
                                    core.jellyfinServerInfo(entry.url)
                                    val quick = runCatching { core.jellyfinQuickConnectStart(entry.url) }.getOrNull()
                                    go(Step.Jellyfin(entry.url, quick))
                                }
                                ServerType.PLEX -> {
                                    presetPlexId = entry.id
                                    go(Step.Plex)
                                }
                            }
                        }
                    },
                    onError = { error = it },
                )

            Step.Direct -> {
                // No AlwyWatch server on this path: forget one picked earlier then backed out of.
                LaunchedEffect(Unit) {
                    core.keepClearPreset()
                    presetPlexId = null
                }
                ChoiceCard("Jellyfin", "Quick Connect or password", Icons.Library, { go(Step.JellyfinUrl) }, Modifier.initialFocus())
                ChoiceCard("Plex", "Sign in on plex.tv", Icons.Play, { go(Step.Plex) })
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
                        go(Step.Jellyfin(normalized, quick))
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
                                // Codes expire after a few minutes (finding a phone takes time on a
                                // TV): get a fresh one instead of giving up.
                                val fresh = runCatching { core.jellyfinQuickConnectStart(s.url) }.getOrNull()
                                if (fresh != null) {
                                    history = history.dropLast(1) + Step.Jellyfin(s.url, fresh)
                                } else {
                                    error = e.message
                                }
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
                                    go(Step.PlexServers(token, servers))
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
    connectedUrl: String? = null,
    signedIn: Set<String> = emptySet(),
    onOther: () -> Unit = {},
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var url by remember { mutableStateOf(connectedUrl.orEmpty()) }
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
    if (connectedUrl != null) {
        // The connected server: its list right away, nothing to type.
        LaunchedEffect(connectedUrl) { load() }
        Text("On your AlwyWatch server (${connectedUrl.substringAfter("://")})", style = MaterialTheme.typography.titleMedium)
        if (entries == null) Text("Loading…", color = Subtle)
    }
    if (connectedUrl == null) {
        Text(
            "Enter the address your server's owner gave you, then pick the server you watch on.",
            color = Muted,
            style = MaterialTheme.typography.bodyMedium,
        )
    }
    if (connectedUrl == null) {
        Field(
            url,
            { url = it },
            placeholder = { Text("watch.example.org") },
            pill = true,
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Go),
            keyboardActions = KeyboardActions(onGo = { load() }),
            modifier = Modifier.fillMaxWidth().dpadEscape().initialFocus(),
        )
        ActionButton("Show servers", { load() }, primary = entries == null)
    }
    entries?.let { (base, list) ->
        if (list.isEmpty()) Text("This AlwyWatch server lists no media servers.", color = Muted)
        list.forEachIndexed { i, e ->
            val plex = e.kind == ServerType.PLEX
            val kind = if (plex) "Plex · sign in on plex.tv" else "Jellyfin · Quick Connect or password"
            ChoiceCard(
                e.name,
                if (e.id in signedIn) "${if (plex) "Plex" else "Jellyfin"} · already signed in" else kind,
                if (plex) Icons.Play else Icons.Library,
                { if (!busy) onPick(base, e) },
                if (i == 0 && connectedUrl != null) Modifier.initialFocus() else Modifier,
            )
        }
    }
    if (connectedUrl != null) {
        Text("Somewhere else", Modifier.padding(top = 8.dp), style = MaterialTheme.typography.titleMedium)
        ChoiceCard("Another server", "Jellyfin or Plex directly, not on your AlwyWatch server", Icons.Add, onOther)
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
