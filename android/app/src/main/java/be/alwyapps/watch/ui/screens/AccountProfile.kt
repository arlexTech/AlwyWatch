// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathFillType
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.keep.Me
import be.alwyapps.watch.services
import be.alwyapps.watch.together.friendly
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Avatar
import be.alwyapps.watch.ui.AvatarImages
import be.alwyapps.watch.ui.AvatarPresets
import be.alwyapps.watch.ui.AvatarRing
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.Chevron
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.DefaultAvatar
import be.alwyapps.watch.ui.Divider
import be.alwyapps.watch.ui.Field
import be.alwyapps.watch.ui.FocusCard
import be.alwyapps.watch.ui.Group
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.ListRow
import be.alwyapps.watch.ui.LocalCore
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.LocalInitialFocusDone
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.PresetAvatar
import be.alwyapps.watch.ui.SurfaceColor
import be.alwyapps.watch.ui.dpadEscape
import be.alwyapps.watch.ui.focusOnAppear
import be.alwyapps.watch.ui.tr
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

@Composable
private fun tvNoUpload() = tr("You can set a custom picture from your phone, computer or the web app.")

/** My AlwyWatch account as the server last said (name, avatar, welcome step done). */
object MyAccount {
    var me by mutableStateOf<Me?>(null)

    /** The welcome step was closed during this run (it comes back at the next start if unfinished). */
    var welcomeClosed by mutableStateOf(false)

    suspend fun refresh(core: AlwyWatch) {
        if (!runCatching { core.keepActive() }.getOrDefault(false)) {
            me = null
            return
        }
        runCatching { core.keepMe() }.onSuccess { me = it }
    }

    /** A new account was just linked: show the welcome step again if it isn't done. */
    suspend fun linked(core: AlwyWatch) {
        welcomeClosed = false
        refresh(core)
    }
}

/** Plain words for a failed profile change. */
private fun problemOf(e: Throwable): String = (e as? CoreException)?.let { friendly(it) } ?: (e.message ?: tr("Something went wrong."))

// ----- Choosing an avatar ---------------------------------------------------------------------

private sealed interface Pick {
    data object Default : Pick

    data class Preset(
        val id: String,
    ) : Pick

    /** The picture already uploaded. */
    data object Current : Pick

    class Upload(
        val bytes: ByteArray,
        val preview: ImageBitmap,
    ) : Pick
}

private fun Me.pick(): Pick =
    when {
        avatar.customVersion != null -> Pick.Current
        AvatarPresets.of(avatar.preset) != null -> Pick.Preset(avatar.preset!!)
        else -> Pick.Default
    }

/**
 * The avatars to choose from: the default (initial), the twelve built-in ones, the uploaded
 * picture if any, and (phones) "Upload a picture".
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun AvatarGrid(
    me: Me,
    name: String,
    pick: Pick,
    onPick: (Pick) -> Unit,
    onUpload: (() -> Unit)?,
    upload: Pick.Upload? = null,
) {
    val d = LocalDimens.current
    val size: Dp = if (d.isTv) 60.dp else 54.dp

    @Composable
    fun Tile(
        selected: Boolean,
        label: String,
        onClick: () -> Unit,
        content: @Composable () -> Unit,
    ) {
        FocusCard(
            onClick = onClick,
            modifier = if (selected) Modifier.focusOnAppear() else Modifier,
            shape = CircleShape,
            glow = false,
        ) {
            Box(Modifier.semanticsLabel(label)) { AvatarRing(selected, size, content) }
        }
    }
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Tile(pick == Pick.Default, tr("Your initial"), { onPick(Pick.Default) }) { DefaultAvatar(name.ifBlank { me.name }, size) }
        for (p in AvatarPresets.all) {
            Tile(pick == Pick.Preset(p.id), tr("Picture {shape}", "shape" to p.shape), { onPick(Pick.Preset(p.id)) }) { PresetAvatar(p, size) }
        }
        if (me.avatar.customVersion != null) {
            Tile(pick == Pick.Current, tr("Your picture"), { onPick(Pick.Current) }) { Avatar(me.avatar, me.name, me.userKey, size) }
        }
        if (upload != null) {
            Tile(pick === upload, tr("New picture"), { onPick(upload) }) {
                Image(upload.preview, null, Modifier.size(size).clip(CircleShape), contentScale = ContentScale.Crop)
            }
        }
        if (onUpload != null) {
            Tile(false, tr("Upload a picture"), onUpload) {
                Box(Modifier.size(size).clip(CircleShape).background(SurfaceColor), contentAlignment = Alignment.Center) {
                    Icon(Icons.Add, null, tint = Muted)
                }
            }
        }
    }
}

private fun Modifier.semanticsLabel(label: String) = semantics { contentDescription = label }

// ----- Settings → AlwyWatch account -----------------------------------------------------------

/** Settings: my display name and avatar on the AlwyWatch server, and changing them. */
@Composable
fun AlwyWatchAccountGroup() {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) { MyAccount.refresh(core) }
    val me = MyAccount.me ?: return
    var editing by remember { mutableStateOf(false) }
    var choosing by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    fun change(block: suspend () -> Me) {
        error = null
        scope.launch {
            try {
                MyAccount.me = block()
            } catch (e: CoreException) {
                error = problemOf(e)
            }
        }
    }
    val upload =
        rememberAvatarUpload { bytes ->
            change {
                core.keepUploadAvatar(bytes, "image/jpeg").also { m ->
                    m.avatar.customVersion?.let { v -> AvatarImages.put(ctx, m.userKey, v, bytes) }
                }
            }
        }
    if (editing) {
        EditNameDialog(me, onClose = { editing = false }) { MyAccount.me = it }
    }
    if (choosing) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { choosing = false },
            containerColor = Background,
            title = { Text(tr("Choose a picture")) },
            text = {
                CompositionLocalProvider(LocalInitialFocusDone provides null) {
                    Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                        AvatarGrid(
                            me,
                            me.name,
                            me.pick(),
                            onPick = { p ->
                                choosing = false
                                when (p) {
                                    is Pick.Preset -> change { core.keepSetAvatarPreset(p.id) }
                                    Pick.Default -> if (me.pick() != Pick.Default) change { core.keepRemoveAvatar() }
                                    else -> {}
                                }
                            },
                            onUpload =
                                if (d.isTv) {
                                    null
                                } else {
                                    {
                                        choosing = false
                                        upload()
                                    }
                                },
                        )
                        if (d.isTv) Text(tvNoUpload(), color = Muted, style = MaterialTheme.typography.bodySmall)
                    }
                }
            },
            confirmButton = { ActionButton(tr("Close"), { choosing = false }) },
            dismissButton =
                if (me.pick() != Pick.Default) {
                    {
                        ActionButton(tr("Remove"), {
                            choosing = false
                            change { core.keepRemoveAvatar() }
                        }, icon = Icons.Delete)
                    }
                } else {
                    null
                },
        )
    }
    Group(tr("Your AlwyWatch account"), footer = tr("Others see this name and picture when you watch together.")) {
        ListRow(
            me.name,
            subtitle =
                listOfNotNull(
                    if (me.nameIsCustom && me.mediaName.isNotEmpty()) tr("Signed in as {name}", "name" to me.mediaName) else tr("How others see you"),
                    // Linked sign-ins: the others, on a second line.
                    alsoSignedInAs(Identity.links),
                ).joinToString("\n"),
            leading = { Avatar(me.avatar, me.name, me.userKey, if (d.isTv) 52.dp else 46.dp) },
        )
        Divider()
        ListRow(tr("Change name"), icon = Icons.Person, onClick = { editing = true }, trailing = { Chevron() })
        if (me.nameIsCustom && me.mediaName.isNotEmpty()) {
            Divider()
            ListRow(tr("Reset to {name}", "name" to me.mediaName), subtitle = tr("Your name on your media server"), icon = Icons.Refresh, onClick = {
                change { core.keepResetDisplayName() }
            })
        }
        Divider()
        ListRow(
            tr("Change picture"),
            subtitle = if (d.isTv) tr("Built-in pictures") else tr("A built-in one, or a photo"),
            icon = Icons.Group,
            onClick = { choosing = true },
            trailing = { Chevron() },
        )
        error?.let { Text(it, Modifier.padding(16.dp), color = Danger, style = MaterialTheme.typography.bodySmall) }
    }
    // One AlwyWatch account for my Plex and Jellyfin sign-ins.
    LinkedSignInsGroup(me)
}

/** The display name, checked as it is typed. */
@Composable
private fun EditNameDialog(
    me: Me,
    onClose: () -> Unit,
    onSaved: (Me) -> Unit,
) {
    val core = LocalCore.current
    val scope = rememberCoroutineScope()
    var name by remember { mutableStateOf(me.name) }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val problem =
        remember(name) {
            be.alwyapps.watch.core
                .displayNameProblem(name)
        }

    fun save() {
        if (busy || problem != null) return
        if (name.trim() == me.name) {
            onClose()
            return
        }
        busy = true
        scope.launch {
            try {
                onSaved(core.keepSetDisplayName(name))
                onClose()
            } catch (e: CoreException) {
                error = problemOf(e)
            } finally {
                busy = false
            }
        }
    }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = onClose,
        containerColor = Background,
        title = { Text(tr("Your name")) },
        text = {
            CompositionLocalProvider(LocalInitialFocusDone provides null) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(tr("This is the name others see."), color = Muted, style = MaterialTheme.typography.bodyMedium)
                    Field(
                        name,
                        {
                            name = it
                            error = null
                        },
                        Modifier.fillMaxWidth().dpadEscape().focusOnAppear(),
                        singleLine = true,
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                        keyboardActions = KeyboardActions(onDone = { save() }),
                    )
                    (error ?: problem?.replaceFirstChar { it.uppercase() })?.let {
                        Text(it, color = Danger, style = MaterialTheme.typography.bodySmall)
                    }
                }
            }
        },
        confirmButton = { ActionButton(if (busy) tr("Saving…") else tr("Save"), { save() }, primary = problem == null) },
        dismissButton = { ActionButton(tr("Cancel"), onClose) },
    )
}

// ----- Welcome step ---------------------------------------------------------------------------

/**
 * Once per account (while the server says it isn't done): how others see you, name and
 * picture. Continue saves what changed, Skip saves nothing; both end the step. Nothing here
 * ever blocks the app: a failed call just closes it (it comes back at the next start).
 */
@Composable
fun ProfileWelcome() {
    val core = LocalCore.current
    LaunchedEffect(Unit) { MyAccount.refresh(core) }
    val me = MyAccount.me ?: return
    if (me.profileSet || MyAccount.welcomeClosed) return
    WelcomeDialog(me)
}

/** The welcome step is on screen (other prompts wait). */
fun welcomeShowing(): Boolean = MyAccount.me?.let { !it.profileSet && !MyAccount.welcomeClosed } ?: false

@Composable
private fun WelcomeDialog(me: Me) {
    val core = LocalCore.current
    val ctx = LocalContext.current
    val d = LocalDimens.current
    var name by remember { mutableStateOf(me.name) }
    var pick by remember { mutableStateOf(me.pick()) }
    var uploaded by remember { mutableStateOf<Pick.Upload?>(null) }
    var busy by remember { mutableStateOf(false) }
    val problem =
        remember(name) {
            be.alwyapps.watch.core
                .displayNameProblem(name)
        }
    val upload =
        rememberAvatarUpload { bytes ->
            val bmp = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
            if (bmp != null) {
                val u = Pick.Upload(bytes, bmp.asImageBitmap())
                uploaded = u
                pick = u
            }
        }

    // Not tied to this dialog: it closes at once, the calls go on.
    fun finish(save: Boolean) {
        if (busy) return
        busy = true
        val chosenName = name.trim()
        val chosen = pick
        MyAccount.welcomeClosed = true
        ctx.services.scope.launch {
            var latest: Me? = null
            if (save) {
                if (chosenName != me.name &&
                    be.alwyapps.watch.core
                        .displayNameProblem(chosenName) == null
                ) {
                    runCatching { core.keepSetDisplayName(chosenName) }.onSuccess { latest = it }
                }
                runCatching {
                    when (chosen) {
                        is Pick.Preset -> if (me.avatar.preset != chosen.id || me.avatar.customVersion != null) core.keepSetAvatarPreset(chosen.id) else null
                        Pick.Default -> if (me.pick() != Pick.Default) core.keepRemoveAvatar() else null
                        is Pick.Upload ->
                            core.keepUploadAvatar(chosen.bytes, "image/jpeg").also { m ->
                                m.avatar.customVersion?.let { v -> AvatarImages.put(ctx, m.userKey, v, chosen.bytes) }
                            }
                        Pick.Current -> null
                    }
                }.getOrNull()?.let { latest = it }
            }
            runCatching { core.keepMarkProfileSet() }.onSuccess { latest = it }
            latest?.let { m -> withContext(Dispatchers.Main) { MyAccount.me = m } }
        }
    }
    Dialog(
        onDismissRequest = { MyAccount.welcomeClosed = true },
        properties = DialogProperties(usePlatformDefaultWidth = false, dismissOnClickOutside = false),
    ) {
        CompositionLocalProvider(LocalInitialFocusDone provides null) {
            Box(Modifier.fillMaxSize().background(Background), contentAlignment = Alignment.TopCenter) {
                Column(
                    Modifier
                        .widthIn(max = 720.dp)
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState())
                        .statusBarsPadding()
                        .navigationBarsPadding()
                        .padding(horizontal = d.gutter, vertical = if (d.isTv) 32.dp else 24.dp),
                    verticalArrangement = Arrangement.spacedBy(18.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(18.dp)) {
                        val big = if (d.isTv) 88.dp else 76.dp
                        when (val p = pick) {
                            is Pick.Upload -> Image(p.preview, null, Modifier.size(big).clip(CircleShape), contentScale = ContentScale.Crop)
                            is Pick.Preset -> AvatarPresets.of(p.id)?.let { PresetAvatar(it, big) }
                            Pick.Current -> Avatar(me.avatar, me.name, me.userKey, big)
                            Pick.Default -> DefaultAvatar(name.ifBlank { me.name }, big)
                        }
                        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            Text(tr("Welcome!"), style = MaterialTheme.typography.displaySmall)
                            Text(tr("This is how others see you"), style = MaterialTheme.typography.bodyLarge, color = Muted)
                        }
                    }
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Field(
                            name,
                            { name = it },
                            Modifier.fillMaxWidth().dpadEscape(),
                            label = { Text(tr("Your name")) },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                        )
                        problem?.let { Text(it.replaceFirstChar { c -> c.uppercase() }, color = Danger, style = MaterialTheme.typography.bodySmall) }
                    }
                    Text(tr("Your picture"), style = MaterialTheme.typography.titleMedium)
                    AvatarGrid(
                        me,
                        name,
                        pick,
                        onPick = { pick = it },
                        onUpload = if (d.isTv) null else upload,
                        upload = uploaded,
                    )
                    if (d.isTv) Text(tvNoUpload(), color = Muted, style = MaterialTheme.typography.bodySmall)
                    Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        ActionButton(tr("Continue"), { finish(save = true) }, Modifier.focusOnAppear(), primary = true, icon = Icons.Check)
                        ActionButton(tr("Skip"), { finish(save = false) })
                    }
                    Text(tr("You can change this later in Settings."), color = Muted, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

// ----- Upload: photo picker, then the cropper ------------------------------------------------

/**
 * The system photo picker (no storage permission), then the cropper; [onCropped] gets the
 * JPEG to upload. Call the returned function to start. Keep it above any dialog that starts it:
 * the result comes back here.
 */
@Composable
private fun rememberAvatarUpload(onCropped: (ByteArray) -> Unit): () -> Unit {
    var source by remember { mutableStateOf<Uri?>(null) }
    val launcher = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri -> source = uri }
    source?.let { uri ->
        AvatarCropper(uri, onCancel = { source = null }) { bytes ->
            source = null
            onCropped(bytes)
        }
    }
    return { launcher.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) }
}

/**
 * Decodes a picked photo upright (EXIF orientation applied) and small enough to handle: its
 * short side at about 1024 px at most (a 48 MP photo is never loaded at full size).
 */
private fun decodeForCrop(
    ctx: Context,
    uri: Uri,
): Bitmap? {
    val cr = ctx.contentResolver
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    cr.openInputStream(uri)?.use { BitmapFactory.decodeStream(it, null, bounds) }
    val w = bounds.outWidth
    val h = bounds.outHeight
    if (w <= 0 || h <= 0) return null
    var sample = 1
    while (min(w, h) / (sample * 2) >= 1024 || max(w, h) / sample > 4096) sample *= 2
    val bmp =
        cr.openInputStream(uri)?.use {
            BitmapFactory.decodeStream(it, null, BitmapFactory.Options().apply { inSampleSize = sample })
        } ?: return null
    val orientation =
        runCatching {
            cr.openInputStream(uri)?.use {
                android.media
                    .ExifInterface(it)
                    .getAttributeInt(android.media.ExifInterface.TAG_ORIENTATION, android.media.ExifInterface.ORIENTATION_NORMAL)
            }
        }.getOrNull() ?: android.media.ExifInterface.ORIENTATION_NORMAL
    val m = android.graphics.Matrix()
    when (orientation) {
        android.media.ExifInterface.ORIENTATION_ROTATE_90 -> m.postRotate(90f)
        android.media.ExifInterface.ORIENTATION_ROTATE_180 -> m.postRotate(180f)
        android.media.ExifInterface.ORIENTATION_ROTATE_270 -> m.postRotate(270f)
        android.media.ExifInterface.ORIENTATION_FLIP_HORIZONTAL -> m.postScale(-1f, 1f)
        android.media.ExifInterface.ORIENTATION_FLIP_VERTICAL -> m.postScale(1f, -1f)
        android.media.ExifInterface.ORIENTATION_TRANSPOSE -> {
            m.postRotate(90f)
            m.postScale(-1f, 1f)
        }
        android.media.ExifInterface.ORIENTATION_TRANSVERSE -> {
            m.postRotate(270f)
            m.postScale(-1f, 1f)
        }
    }
    if (m.isIdentity) return bmp
    val upright = Bitmap.createBitmap(bmp, 0, 0, bmp.width, bmp.height, m, true)
    if (upright !== bmp) bmp.recycle()
    return upright
}

/** The square under the frame, at most 512 px (never upscaled), as JPEG (under 512 KiB). */
private fun renderCrop(
    src: Bitmap,
    left: Float,
    top: Float,
    side: Float,
): ByteArray {
    val out = min(512, side.roundToInt()).coerceAtLeast(1)
    val bmp = Bitmap.createBitmap(out, out, Bitmap.Config.ARGB_8888)
    android.graphics.Canvas(bmp).drawBitmap(
        src,
        android.graphics.Rect(left.roundToInt(), top.roundToInt(), (left + side).roundToInt(), (top + side).roundToInt()),
        android.graphics.Rect(0, 0, out, out),
        android.graphics.Paint(android.graphics.Paint.FILTER_BITMAP_FLAG),
    )

    fun encode(quality: Int) =
        java.io.ByteArrayOutputStream().use { s ->
            bmp.compress(Bitmap.CompressFormat.JPEG, quality, s)
            s.toByteArray()
        }
    val bytes = encode(88).let { if (it.size > 500 * 1024) encode(70) else it }
    bmp.recycle()
    return bytes
}

/**
 * The picture under a round frame: drag to move it, pinch (or the slider) to zoom; the frame is
 * always covered. Save renders the square under the frame.
 */
@Composable
private fun AvatarCropper(
    uri: Uri,
    onCancel: () -> Unit,
    onSave: (ByteArray) -> Unit,
) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    var bitmap by remember(uri) { mutableStateOf<Bitmap?>(null) }
    var failed by remember(uri) { mutableStateOf(false) }
    var saving by remember { mutableStateOf(false) }
    LaunchedEffect(uri) {
        val b = withContext(Dispatchers.IO) { runCatching { decodeForCrop(ctx, uri) }.getOrNull() }
        if (b == null) failed = true else bitmap = b
    }
    // Zoom (1 = the frame just covered) and where the picture's centre sits from the frame's.
    var zoom by remember(uri) { mutableFloatStateOf(1f) }
    var offset by remember(uri) { mutableStateOf(Offset.Zero) }
    Dialog(
        onDismissRequest = onCancel,
        properties = DialogProperties(usePlatformDefaultWidth = false),
    ) {
        Column(
            Modifier
                .fillMaxSize()
                .background(Color.Black)
                .statusBarsPadding()
                .navigationBarsPadding(),
        ) {
            Text(
                tr("Move and zoom your picture"),
                Modifier.padding(horizontal = 20.dp, vertical = 16.dp),
                style = MaterialTheme.typography.titleMedium,
                color = Color.White,
            )
            val src = bitmap
            BoxWithConstraints(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                val density = LocalDensity.current
                val boxW = with(density) { maxWidth.toPx() }
                val boxH = with(density) { maxHeight.toPx() }
                val frame = min(boxW, boxH) * 0.82f
                when {
                    failed -> Text(tr("Couldn't open this picture."), color = Color.White)
                    src == null -> Text(tr("Opening…"), color = Muted)
                    else -> {
                        val image = remember(src) { src.asImageBitmap() }
                        val w = src.width.toFloat()
                        val h = src.height.toFloat()
                        val base = frame / min(w, h)

                        fun clamp(
                            o: Offset,
                            z: Float,
                        ): Offset {
                            val s = base * z
                            val mx = ((w * s - frame) / 2).coerceAtLeast(0f)
                            val my = ((h * s - frame) / 2).coerceAtLeast(0f)
                            return Offset(o.x.coerceIn(-mx, mx), o.y.coerceIn(-my, my))
                        }
                        Canvas(
                            Modifier
                                .fillMaxSize()
                                .pointerInput(src, frame) {
                                    detectTransformGestures { centroid, pan, gestureZoom, _ ->
                                        val nz = (zoom * gestureZoom).coerceIn(1f, MAX_ZOOM)
                                        val k = nz / zoom
                                        // Zoom about the fingers, then follow them.
                                        val rel = centroid - Offset(size.width / 2f, size.height / 2f)
                                        offset = clamp((offset - rel) * k + rel + pan, nz)
                                        zoom = nz
                                    }
                                },
                        ) {
                            val s = base * zoom
                            val c = center + offset
                            drawImage(
                                image,
                                dstOffset = IntOffset((c.x - w * s / 2).roundToInt(), (c.y - h * s / 2).roundToInt()),
                                dstSize = IntSize((w * s).roundToInt(), (h * s).roundToInt()),
                            )
                            val circle = Rect(center, frame / 2)
                            val shade =
                                Path().apply {
                                    fillType = PathFillType.EvenOdd
                                    addRect(Rect(Offset.Zero, size))
                                    addOval(circle)
                                }
                            drawPath(shade, Color.Black.copy(alpha = 0.6f))
                            drawCircle(Color.White, frame / 2, center, style = Stroke(2.dp.toPx()))
                        }
                        // Save: the square under the frame, in the picture's own pixels.
                        SaveTrigger(saving) {
                            val sc = base * zoom
                            val side = frame / sc
                            val cx = w / 2 - offset.x / sc
                            val cy = h / 2 - offset.y / sc
                            val left = (cx - side / 2).coerceIn(0f, (w - side).coerceAtLeast(0f))
                            val top = (cy - side / 2).coerceIn(0f, (h - side).coerceAtLeast(0f))
                            scope.launch {
                                val bytes = withContext(Dispatchers.Default) { runCatching { renderCrop(src, left, top, min(side, min(w, h))) }.getOrNull() }
                                saving = false
                                if (bytes == null) failed = true else onSave(bytes)
                            }
                        }
                        ZoomSlider(zoom) { nz ->
                            offset = clamp(offset * (nz / zoom), nz)
                            zoom = nz
                        }
                    }
                }
            }
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 16.dp),
                horizontalArrangement = Arrangement.spacedBy(12.dp, Alignment.End),
            ) {
                ActionButton(tr("Cancel"), onCancel)
                ActionButton(if (saving) tr("Saving…") else tr("Save"), {
                    if (bitmap != null && !saving) saving = true
                }, primary = true, icon = Icons.Check)
            }
        }
    }
}

private const val MAX_ZOOM = 5f

/** Runs [save] once when [saving] turns on (it reads the cropper's current frame). */
@Composable
private fun SaveTrigger(
    saving: Boolean,
    save: () -> Unit,
) {
    LaunchedEffect(saving) { if (saving) save() }
}

/** Zoom slider over the bottom of the picture. */
@Composable
private fun androidx.compose.foundation.layout.BoxScope.ZoomSlider(
    zoom: Float,
    onZoom: (Float) -> Unit,
) {
    androidx.compose.material3.Slider(
        value = zoom,
        onValueChange = onZoom,
        valueRange = 1f..MAX_ZOOM,
        modifier =
            Modifier
                .align(Alignment.BottomCenter)
                .widthIn(max = 420.dp)
                .fillMaxWidth()
                .padding(horizontal = 32.dp, vertical = 8.dp),
    )
}
