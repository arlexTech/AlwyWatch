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

/** Open for this process once the profile was picked (and its PIN checked). */
object ProfileSession {
    var open = false
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
        mutableStateOf(ProfileSession.open || unlocked || (profiles.size < 2 && current?.hasPin != true))
    }
    if (open) {
        ProfileSession.open = true
        content()
        return
    }
    WhoIsWatching(onPicked = { id ->
        if (id == services.profile) {
            ProfileSession.open = true
            open = true
        } else {
            services.switchProfile(ctx as Activity, id, alreadyUnlocked = true)
        }
    })
}

@Composable
private fun WhoIsWatching(onPicked: (String) -> Unit) {
    val ctx = LocalContext.current
    val store = ctx.services.profiles
    val profiles = remember { store.list() }
    var asking by remember { mutableStateOf<Profile?>(if (profiles.size == 1) profiles.single() else null) }
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
            Text("Who's watching?", style = MaterialTheme.typography.displaySmall)
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
            if (p.hasPin) Text("PIN", style = MaterialTheme.typography.labelSmall, color = Muted)
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
        Text(name.firstOrNull()?.uppercase() ?: "?", color = Color.Black, style = MaterialTheme.typography.headlineMedium)
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
    Text("PIN for ${p.name}", style = MaterialTheme.typography.titleLarge)
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
    if (wrong > 0) Text("Wrong PIN", color = Danger)
    onBack?.let { ActionButton("Back", it) }
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
    var adding by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf<Profile?>(null) }
    var pinFor by remember { mutableStateOf<Profile?>(null) }
    var removing by remember { mutableStateOf<Profile?>(null) }
    var switching by remember { mutableStateOf<Profile?>(null) }

    Group("Profiles", footer = "Several people on this device: each profile has its own servers, history and settings. Downloads are shared.") {
        profiles.forEachIndexed { i, p ->
            if (i > 0) Divider()
            ListRow(
                p.name,
                subtitle = if (p.hasPin) "Protected by a PIN" else null,
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
                            Value("In use")
                            be.alwyapps.watch.ui
                                .IconAction(Icons.Settings, "PIN", { pinFor = p })
                        } else {
                            be.alwyapps.watch.ui
                                .IconAction(Icons.Delete, "Remove ${p.name}", { removing = p })
                        }
                    }
                },
            )
        }
        Divider()
        ListRow("Add a profile", icon = Icons.Add, onClick = { adding = true }, trailing = { Chevron() })
    }

    if (adding || renaming != null) {
        var name by remember { mutableStateOf(renaming?.name ?: "") }
        androidx.compose.material3.AlertDialog(
            onDismissRequest = {
                adding = false
                renaming = null
            },
            containerColor = Background,
            title = { Text(if (adding) "New profile" else "Rename") },
            text = {
                Field(name, { name = it.take(24) }, Modifier.fillMaxWidth(), placeholder = { Text("Name") }, singleLine = true)
            },
            confirmButton = {
                ActionButton(if (adding) "Create and switch" else "Save", {
                    val r = renaming
                    if (r != null) {
                        services.profiles.rename(r.id, name)
                        renaming = null
                        version++
                    } else {
                        // A new profile starts empty: its own sign-in.
                        val id = services.profiles.add(name)
                        services.switchProfile(ctx as Activity, id, alreadyUnlocked = true)
                    }
                }, primary = true)
            },
            dismissButton = {
                ActionButton("Cancel", {
                    adding = false
                    renaming = null
                })
            },
        )
    }

    pinFor?.let { p ->
        var pin by remember { mutableStateOf("") }
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { pinFor = null },
            containerColor = Background,
            title = { Text(if (p.hasPin) "PIN for ${p.name}" else "Protect ${p.name} with a PIN") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    be.alwyapps.watch.ui
                        .NoAutofill()
                    Text("Asked when opening this profile.", color = Muted)
                    Field(
                        pin,
                        { pin = it.filter(Char::isDigit).take(4) },
                        Modifier.fillMaxWidth(),
                        placeholder = { Text(if (p.hasPin) "New 4-digit PIN" else "4-digit PIN") },
                        singleLine = true,
                        visualTransformation = PasswordVisualTransformation(),
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                    )
                }
            },
            confirmButton = {
                ActionButton("Save", {
                    if (pin.length == 4) {
                        services.profiles.setPin(p.id, pin)
                        pinFor = null
                        version++
                    }
                }, primary = true)
            },
            dismissButton = {
                if (p.hasPin) {
                    ActionButton("Remove PIN", {
                        services.profiles.setPin(p.id, null)
                        pinFor = null
                        version++
                    })
                } else {
                    ActionButton("Cancel", { pinFor = null })
                }
            },
        )
    }

    switching?.let { p ->
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { switching = null },
            containerColor = Background,
            title = { Text("Switch to ${p.name}") },
            text = {
                Column(
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) { PinEntry(p, null) { services.switchProfile(ctx as Activity, p.id, alreadyUnlocked = true) } }
            },
            confirmButton = { ActionButton("Cancel", { switching = null }) },
        )
    }

    removing?.let { p ->
        ConfirmDialog(
            "Remove ${p.name}?",
            "Their servers, history and settings on this device are deleted. Downloads only they wanted are deleted too.",
            confirm = "Remove",
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
