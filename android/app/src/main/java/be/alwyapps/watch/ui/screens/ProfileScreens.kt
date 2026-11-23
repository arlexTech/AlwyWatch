// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.app.Activity
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import be.alwyapps.watch.data.Profile
import be.alwyapps.watch.services
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.ConfirmDialog
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Value
import be.alwyapps.watch.ui.ambientGlow
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.initialFocus
import be.alwyapps.watch.ui.tr

/** Open for this process once the profile was picked (and its PIN checked). */
object ProfileSession {
    var open = false

    /** "Switch" from the menu: show "Who's watching?" even with automatic sign-in. */
    var picker = false
}

/**
 * Before the app: "Who's watching?" when the device has several profiles, or the PIN of the
 * only profile when it has one. Picking another profile restarts the app into it.
 */
@Composable
fun ProfileGate(content: @Composable () -> Unit) {
    val ctx = LocalContext.current
    val services = ctx.services
    var open by remember {
        val profiles = services.profiles.list()
        val current = profiles.firstOrNull { it.id == services.profile }
        // Coming from a switch whose PIN was already checked.
        val unlocked = services.profiles.unlockedOnce == services.profile
        if (unlocked) services.profiles.unlockedOnce = null
        // Automatic sign-in: straight into the last profile (its PIN is still asked).
        val auto = services.profiles.autoSignIn && current?.hasPin != true
        val picker = ProfileSession.picker
        ProfileSession.picker = false
        mutableStateOf(!picker && (ProfileSession.open || unlocked || auto || (profiles.size < 2 && current?.hasPin != true)))
    }
    if (open) {
        ProfileSession.open = true
        content()
        return
    }
    val autoPin =
        remember {
            services.profiles.autoSignIn &&
                services.profiles
                    .list()
                    .firstOrNull { it.id == services.profile }
                    ?.hasPin == true
        }
    WhoIsWatching(only = if (autoPin) services.profile else null, onPicked = { id ->
        if (id == services.profile) {
            ProfileSession.open = true
            open = true
        } else {
            services.switchProfile(ctx as Activity, id, alreadyUnlocked = true)
        }
    })
}

@Composable
private fun WhoIsWatching(
    only: String? = null,
    onPicked: (String) -> Unit,
) {
    val ctx = LocalContext.current
    val store = ctx.services.profiles
    val profiles = remember { store.list() }
    // One profile, or automatic sign-in into a PIN-protected one: just its PIN.
    var asking by remember {
        mutableStateOf<Profile?>(profiles.firstOrNull { it.id == only } ?: if (profiles.size == 1) profiles.single() else null)
    }
    Column(
        Modifier
            .fillMaxSize()
            .background(Background)
            .ambientGlow(Accent.copy(alpha = 0.45f))
            .safeDrawingPadding()
            .padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(28.dp, Alignment.CenterVertically),
    ) {
        val a = asking
        if (a == null) {
            Text(tr("Who's watching?"), style = MaterialTheme.typography.displaySmall)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(20.dp, Alignment.CenterHorizontally), verticalArrangement = Arrangement.spacedBy(20.dp)) {
                profiles.forEachIndexed { i, p ->
                    ProfileTile(p, if (i == 0) Modifier.initialFocus() else Modifier) {
                        if (p.hasPin) asking = p else onPicked(p.id)
                    }
                }
            }
        } else {
            PinEntry(a, onBack = if (profiles.size > 1) ({ asking = null }) else null) { onPicked(a.id) }
        }
    }
}

@Composable
private fun ProfileTile(
    p: Profile,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    // The whole tile (avatar and name) is the target.
    FocusCard(
        onClick = onClick,
        modifier = modifier,
        shape =
            androidx.compose.foundation.shape
                .RoundedCornerShape(20.dp),
        glow = false,
    ) {
        Column(
            Modifier.width(120.dp).padding(vertical = 8.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Avatar(p.name, 96)
            Text(p.name, style = MaterialTheme.typography.titleSmall, maxLines = 1, textAlign = TextAlign.Center)
            if (p.hasPin) Text(tr("PIN"), style = MaterialTheme.typography.labelSmall, color = Muted)
        }
    }
}

/** Round initial with a colour derived from the name. */
@Composable
fun Avatar(
    name: String,
    size: Int,
) {
    val palette =
        listOf(
            listOf(Color(0xFFFFB224), Color(0xFFFF6A3D)),
            listOf(Color(0xFF5B8CFF), Color(0xFF9B5BFF)),
            listOf(Color(0xFF3DDC97), Color(0xFF14A3C7)),
            listOf(Color(0xFFFF5A8A), Color(0xFFFF9F5A)),
        )
    val colors = palette[(name.hashCode() and 0x7fffffff) % palette.size]
    Box(
        Modifier
            .size(size.dp)
            .clip(CircleShape)
            .background(Brush.linearGradient(colors)),
        contentAlignment = Alignment.Center,
    ) {
        // Small ones (a menu) scale their letter; the tiles keep the headline size.
        val style = MaterialTheme.typography.headlineMedium
        Text(
            name.firstOrNull()?.uppercase() ?: "?",
            color = Color.Black,
            style = if (size < 48) style.copy(fontSize = (size * 0.5f).sp, lineHeight = (size * 0.6f).sp) else style,
        )
    }
}

@Composable
private fun PinEntry(
    p: Profile,
    onBack: (() -> Unit)?,
    onOk: () -> Unit,
) {
    val store = LocalContext.current.services.profiles
    be.alwyapps.watch.ui
        .NoAutofill()
    var pin by remember { mutableStateOf("") }
    var wrong by remember { mutableIntStateOf(0) }

    fun check() {
        if (store.checkPin(p.id, pin)) {
            onOk()
        } else {
            wrong++
            pin = ""
        }
    }
    Avatar(p.name, 96)
    Text(tr("PIN for {name}", "name" to p.name), style = MaterialTheme.typography.titleLarge)
    Field(
        pin,
        {
            pin = it.filter(Char::isDigit).take(4)
            if (pin.length == 4) check()
        },
        Modifier.width(220.dp).dpadEscape().initialFocus(),
        placeholder = { Text("••••") },
        singleLine = true,
        visualTransformation = PasswordVisualTransformation(),
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { check() }),
    )
    if (wrong > 0) Text(tr("Wrong PIN"), color = Danger)
    onBack?.let { ActionButton(tr("Back"), it) }
}

/**
 * The profile in use. The first one starts without a name of its own ("Me"): it takes its
 * account's (the AlwyWatch account, else the media account) the first time one is there.
 */
fun currentProfile(
    services: be.alwyapps.watch.Services,
    core: be.alwyapps.watch.core.AlwyWatch,
): Profile? {
    val p = services.profiles.list().firstOrNull { it.id == services.profile } ?: return null
    if (!p.unnamed) return p
    val name =
        runCatching {
            core.account()?.name?.takeIf { it.isNotBlank() } ?: core.accounts().let { all ->
                val current = core.getSetting("current_account")
                (all.firstOrNull { it.id == current } ?: all.firstOrNull())?.userName
            }
        }.getOrNull()?.trim()?.take(24)
    if (name.isNullOrEmpty()) return p
    services.profiles.rename(p.id, name)
    return p.copy(name = name)
}

/**
 * "Who's watching?" from anywhere (the avatar on the phone's home, the side menu): one tap on a
 * profile switches to it (its PIN is asked here when it has one), with a way to add one and to
 * the settings.
 */
@Composable
fun ProfileSwitcher(
    onDismiss: () -> Unit,
    onSettings: (() -> Unit)? = null,
) {
    val ctx = LocalContext.current
    val services = ctx.services
    val profiles = remember { services.profiles.list() }
    var asking by remember { mutableStateOf<Profile?>(null) }
    val switchTo = { id: String -> services.switchProfile(ctx as Activity, id, alreadyUnlocked = true) }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Background,
        title = {
            Text(
                when {
                    asking != null -> tr("Switch to {name}", "name" to asking?.name)
                    else -> tr("Who's watching?")
                },
            )
        },
        text = {
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
                val a = asking
                when {
                    a != null -> PinEntry(a, null) { switchTo(a.id) }
                    else ->
                        FlowRow(
                            horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterHorizontally),
                            verticalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            profiles.forEachIndexed { i, p ->
                                val current = p.id == services.profile
                                SwitcherTile(
                                    p.name,
                                    if (current) {
                                        tr("Current")
                                    } else if (p.hasPin) {
                                        tr("PIN")
                                    } else {
                                        ""
                                    },
                                    if (i ==
                                        0
                                    ) {
                                        Modifier.initialFocus()
                                    } else {
                                        Modifier
                                    },
                                ) {
                                    when {
                                        current -> onDismiss()
                                        p.hasPin -> asking = p
                                        else -> switchTo(p.id)
                                    }
                                }
                            }
                            SwitcherTile(tr("Add"), "", Modifier, add = true) { switchTo(addProfile(services)) }
                        }
                }
            }
        },
        confirmButton = {
            when {
                asking != null -> ActionButton(tr("Back"), { asking = null })
                onSettings != null -> ActionButton(tr("Settings"), onSettings, icon = Icons.Settings)
                else -> ActionButton(tr("Close"), onDismiss)
            }
        },
    )
}

/**
 * A new, empty profile (one person): it opens on "Add a profile" (sign in to AlwyWatch, create
 * an account, or Jellyfin or Plex only) and takes its name from the first account there.
 */
fun addProfile(services: be.alwyapps.watch.Services): String = services.profiles.add(be.alwyapps.watch.data.UNNAMED)

@Composable
private fun SwitcherTile(
    name: String,
    note: String,
    modifier: Modifier,
    add: Boolean = false,
    onClick: () -> Unit,
) {
    FocusCard(
        onClick = onClick,
        modifier = modifier,
        shape =
            androidx.compose.foundation.shape
                .RoundedCornerShape(18.dp),
        glow = false,
    ) {
        Column(
            Modifier.width(92.dp).padding(vertical = 8.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            if (add) {
                Box(Modifier.size(64.dp).clip(CircleShape).background(be.alwyapps.watch.ui.SurfaceHigh), contentAlignment = Alignment.Center) {
                    androidx.compose.material3.Icon(Icons.Add, null, tint = Color.White)
                }
            } else {
                Avatar(name, 64)
            }
            Text(name, style = MaterialTheme.typography.titleSmall, maxLines = 1, textAlign = TextAlign.Center)
            Text(note, style = MaterialTheme.typography.labelSmall, color = Muted, maxLines = 1)
        }
    }
}

/** Settings: the device's profiles (switch, add, rename, PIN, remove). */
@Composable
fun ProfilesGroup() {
    val ctx = LocalContext.current
    val core = LocalCore.current
    val services = ctx.services
    var version by remember { mutableIntStateOf(0) }
    val profiles = remember(version) { services.profiles.list() }
    val current = services.profile
    var renaming by remember { mutableStateOf<Profile?>(null) }
    var pinFor by remember { mutableStateOf<Profile?>(null) }
    var removing by remember { mutableStateOf<Profile?>(null) }
    var switching by remember { mutableStateOf<Profile?>(null) }

    Group(
        tr("Profiles"),
        footer = tr("A profile is one person: their own AlwyWatch account or logins, history and settings. Downloads are shared."),
    ) {
        profiles.forEachIndexed { i, p ->
            if (i > 0) Divider()
            ListRow(
                p.name,
                subtitle = if (p.hasPin) tr("Protected by a PIN") else null,
                leading = { Avatar(p.name, 34) },
                onClick =
                    if (p.id == current) {
                        { renaming = p }
                    } else {
                        { if (p.hasPin) switching = p else services.switchProfile(ctx as Activity, p.id, alreadyUnlocked = true) }
                    },
                trailing = {
                    androidx.compose.foundation.layout.Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (p.id == current) {
                            Value(tr("In use"))
                            be.alwyapps.watch.ui
                                .IconAction(Icons.Settings, tr("PIN"), { pinFor = p })
                        } else {
                            be.alwyapps.watch.ui
                                .IconAction(Icons.Delete, tr("Remove {name}", "name" to p.name), { removing = p })
                        }
                    }
                },
            )
        }
        Divider()
        ListRow(
            tr("Add a profile"),
            subtitle = tr("Someone else on this device"),
            icon = Icons.Add,
            onClick = { services.switchProfile(ctx as Activity, addProfile(services), alreadyUnlocked = true) },
            trailing = { Chevron() },
        )
        if (profiles.size > 1) {
            Divider()
            var auto by remember { mutableStateOf(services.profiles.autoSignIn) }
            be.alwyapps.watch.ui.ToggleRow(
                tr("Skip “Who's watching?” at start"),
                auto,
                {
                    auto = it
                    services.profiles.autoSignIn = it
                },
                subtitle = tr("Open the last profile used instead of asking who's watching (a PIN is still asked)"),
            )
        }
    }

    renaming?.let { r ->
        var name by remember { mutableStateOf(r.name) }
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { renaming = null },
            containerColor = Background,
            title = { Text(tr("Rename")) },
            text = {
                Field(name, { name = it.take(24) }, Modifier.fillMaxWidth(), placeholder = { Text(tr("Name")) }, singleLine = true)
            },
            confirmButton = {
                ActionButton(tr("Save"), {
                    services.profiles.rename(r.id, name)
                    renaming = null
                    version++
                }, primary = true)
            },
            dismissButton = { ActionButton(tr("Cancel"), { renaming = null }) },
        )
    }

    pinFor?.let { p ->
        var pin by remember { mutableStateOf("") }
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { pinFor = null },
            containerColor = Background,
            title = { Text(if (p.hasPin) tr("PIN for {name}", "name" to p.name) else tr("Protect {name} with a PIN", "name" to p.name)) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    be.alwyapps.watch.ui
                        .NoAutofill()
                    Text(tr("Asked when opening this profile."), color = Muted)
                    Field(
                        pin,
                        { pin = it.filter(Char::isDigit).take(4) },
                        Modifier.fillMaxWidth(),
                        placeholder = { Text(if (p.hasPin) tr("New 4-digit PIN") else tr("4-digit PIN")) },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                    )
                }
            },
            confirmButton = {
                ActionButton(tr("Save"), {
                    if (pin.length == 4) {
                        services.profiles.setPin(p.id, pin)
                        pinFor = null
                        version++
                    }
                }, primary = true)
            },
            dismissButton = {
                if (p.hasPin) {
                    ActionButton(tr("Remove PIN"), {
                        services.profiles.setPin(p.id, null)
                        pinFor = null
                        version++
                    })
                } else {
                    ActionButton(tr("Cancel"), { pinFor = null })
                }
            },
        )
    }

    switching?.let { p ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { switching = null },
            containerColor = Background,
            title = { Text(tr("Switch to {name}", "name" to p.name)) },
            text = {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) { PinEntry(p, null) { services.switchProfile(ctx as Activity, p.id, alreadyUnlocked = true) } }
            },
            confirmButton = { ActionButton(tr("Cancel"), { switching = null }) },
        )
    }

    removing?.let { p ->
        ConfirmDialog(
            tr("Remove {name}?", "name" to p.name),
            tr("Their servers, history and settings on this device are deleted. Downloads only they wanted are deleted too."),
            confirm = tr("Remove"),
            onConfirm = {
                runCatching { core.releaseProfileDownloads(p.id) }
                java.io.File(ctx.filesDir, "profiles/${p.id}").deleteRecursively()
                ctx.deleteSharedPreferences("tokens-${p.id}")
                services.profiles.remove(p.id)
                version++
            },
            onDismiss = { removing = null },
        )
    }
}
