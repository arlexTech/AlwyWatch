// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.player

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.focusable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsFocusedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.media3.common.C
import androidx.media3.common.Player
import androidx.media3.common.TrackSelectionOverride
import be.alwyapps.watch.together.people
import be.alwyapps.watch.ui.Accent
import be.alwyapps.watch.ui.I18n
import be.alwyapps.watch.ui.Icons
import be.alwyapps.watch.ui.LocalDimens
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.Subtle
import be.alwyapps.watch.ui.tr
import be.alwyapps.watch.ui.trn
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/** Overlay state shared between the activity (keys, player events) and the Compose controls. */
class PlayerUi {
    private var shown by mutableStateOf(true)

    /** Controls shown. Hiding records when, so a Back pressed right after isn't taken as "leave". */
    var visible: Boolean
        get() = shown
        set(v) {
            if (shown && !v) hiddenAt = android.os.SystemClock.uptimeMillis()
            shown = v
        }

    /** Uptime (ms) the controls last hid. */
    var hiddenAt = 0L
        private set

    /** Hidden on purpose (Back, tap): no grace period, the next Back leaves. */
    fun dismiss() {
        shown = false
        hiddenAt = 0L
    }

    var tracksOpen by mutableStateOf(false)

    /**
     * Playing the server's conversion: the file's tracks and the ones it carries (its stream
     * has one audio track and burned-in subtitles, so the panel lists the file's and a pick
     * asks the server again). Null for direct play (the player's own tracks).
     */
    var serverTracks by mutableStateOf<ServerTracks?>(null)
    var title by mutableStateOf("")
    var subtitle by mutableStateOf("")
    var hasNext by mutableStateOf(false)
    var hasPrevious by mutableStateOf(false)

    /** "Skip intro", "Skip credits"… while inside a skippable segment, else null. */
    var skipLabel by mutableStateOf<String?>(null)

    /** Seconds before the next episode starts by itself (during the credits), else null. */
    var upNextIn by mutableStateOf<Int?>(null)
    var playing by mutableStateOf(false)
    var buffering by mutableStateOf(true)

    /** In a picture-in-picture window: no controls at all (the system draws its own). */
    var pip by mutableStateOf(false)

    /** Last seconds of an episode with a next one: show the "Next episode" prompt. */
    var nearEnd by mutableStateOf(false)

    /** Watch together: the room (null: watching alone, nothing shown) and who is in it. */
    var togetherCode by mutableStateOf<String?>(null)
    var togetherMembers by mutableStateOf<List<be.alwyapps.watch.core.keep.TogetherMember>>(emptyList())

    /** Watching alone with an AlwyWatch server: "Watch together" opens a room with this title. */
    var canWatchTogether by mutableStateOf(false)

    /** A short line about the room ("Ana paused"), shown a few seconds even with hidden controls. */
    var togetherNote by mutableStateOf<String?>(null)

    /** The room waits before playing ("Waiting for Bo…"): shown as long as it lasts. */
    var togetherWaiting by mutableStateOf<String?>(null)

    /** "Watch together" pressed while watching alone: the room dialog over the paused video. */
    var roomDialog by mutableStateOf(false)

    /** Seeking with ←/→ while the controls are hidden: where it goes, and a counter per press. */
    var seekTarget by mutableStateOf<Long?>(null)
    var seekFlash by mutableIntStateOf(0)

    /** The quality menu; None for a downloaded file (nothing to convert). */
    var quality by mutableStateOf<QualityInfo?>(null)

    /**
     * The "+30" / "−10" bubble (double taps, remote, media keys): its side (true = forward)
     * and the seconds added up while presses follow each other, bumped once per press.
     */
    var seekBubble by mutableStateOf<Pair<Boolean, Int>?>(null)
    var seekBubbleN by mutableIntStateOf(0)
    private var seekBubbleAt = 0L

    /** Whether a press on that side now adds to the bubble shown (taps that follow a double tap). */
    fun seekBubbleActive(forward: Boolean) = seekBubble?.first == forward && android.os.SystemClock.uptimeMillis() - seekBubbleAt < SEEK_BUBBLE_MS

    fun bubble(
        forward: Boolean,
        seconds: Int,
    ) {
        val soFar = if (seekBubbleActive(forward)) seekBubble?.second ?: 0 else 0
        seekBubble = forward to soFar + seconds
        seekBubbleAt = android.os.SystemClock.uptimeMillis()
        seekBubbleN++
    }

    /** Paused after hours without the remote: "Are you still watching?". */
    var stillWatching by mutableStateOf(false)

    /** Bumped on every interaction: restarts the auto-hide timer. */
    var touch by mutableIntStateOf(0)

    fun poke() {
        visible = true
        touch++
    }
}

/** The quality menu: what plays now, the choices (original first), the size picked. */
data class QualityInfo(
    val label: String,
    val choices: List<be.alwyapps.watch.core.media.QualityChoice>,
    val chosen: be.alwyapps.watch.core.media.Quality?,
)

/** The file's streams and the selection the conversion was asked with. */
data class ServerTracks(
    val streams: List<be.alwyapps.watch.core.media.MediaStream>,
    val selection: be.alwyapps.watch.core.media.TrackSelection,
)

fun formatTime(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    val h = s / 3600
    val m = (s % 3600) / 60
    val sec = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}

@Composable
fun PlayerOverlay(
    ui: PlayerUi,
    player: () -> Player?,
    onBack: () -> Unit,
    onNext: () -> Unit,
    onPrevious: () -> Unit,
    onSkip: () -> Unit,
    onPip: (() -> Unit)? = null,
    onLeaveTogether: () -> Unit = {},
    onStartTogether: () -> Unit = {},
    onTrackPicked: (Int, androidx.media3.common.Format?) -> Unit = { _, _ -> },
    onServerTrackPicked: (be.alwyapps.watch.core.media.StreamKind, Long?) -> Unit = { _, _ -> },
    onQualityPicked: (be.alwyapps.watch.core.media.Quality?) -> Unit = {},
    onStillWatching: () -> Unit = {},
) {
    if (ui.stillWatching && !ui.pip) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = onStillWatching,
            containerColor = be.alwyapps.watch.ui.Background,
            title = { Text(tr("Are you still watching?")) },
            text = { Text(ui.title, color = Muted) },
            confirmButton = {
                be.alwyapps.watch.ui
                    .ActionButton(tr("Continue"), onStillWatching, primary = true)
            },
            dismissButton = {
                be.alwyapps.watch.ui
                    .ActionButton(tr("Stop"), onBack)
            },
        )
    }
    if (ui.pip) return
    var roomDialog by remember { mutableStateOf(false) }
    if (roomDialog) {
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { roomDialog = false },
            containerColor = be.alwyapps.watch.ui.Background,
            title = { Text(tr("Watch together")) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text(tr("Watching now"), color = Muted, style = MaterialTheme.typography.bodySmall)
                    for ((m, devices) in ui.togetherMembers.people()) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                            be.alwyapps.watch.ui
                                .Avatar(m.avatar, m.name, m.userKey, 36.dp)
                            Text(
                                if (devices > 1) trn("{name} · {n} device", "{name} · {n} devices", devices, "name" to m.name) else m.name,
                                style = MaterialTheme.typography.titleMedium,
                                maxLines = 1,
                            )
                        }
                    }
                    Text(
                        tr("Play, pause and seeking follow everyone. Leaving is for good: you'd need a new invitation to come back."),
                        color = Muted,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            },
            confirmButton = {
                be.alwyapps.watch.ui
                    .ActionButton(tr("Close"), { roomDialog = false }, primary = true)
            },
            dismissButton = {
                be.alwyapps.watch.ui.ActionButton(tr("Leave"), {
                    roomDialog = false
                    onLeaveTogether()
                })
            },
        )
    }
    val tv = LocalDimens.current.isTv
    var position by remember { mutableLongStateOf(0L) }
    var duration by remember { mutableLongStateOf(0L) }
    var buffered by remember { mutableLongStateOf(0L) }
    // Positions refresh only while the controls are shown.
    LaunchedEffect(ui.visible) {
        while (ui.visible) {
            player()?.let {
                position = it.currentPosition
                duration = it.duration.coerceAtLeast(0)
                buffered = it.bufferedPosition
            }
            delay(250)
        }
    }
    // Auto-hide after 4 s without interaction while playing.
    LaunchedEffect(ui.touch, ui.playing, ui.tracksOpen) {
        if (ui.playing && !ui.tracksOpen) {
            delay(4000)
            ui.visible = false
        }
    }
    val playFocus = remember { FocusRequester() }
    // Touch: a double tap on the left half goes back 10 s, on the right half forward 30 s.
    // Each further tap on the same side soon after repeats it, so taps add up (−10, −20…),
    // counted in the bubble on that side.
    val seekByTap = { forward: Boolean ->
        player()?.let { p ->
            val step = if (forward) p.seekForwardIncrement else -p.seekBackIncrement
            val base = ui.seekTarget ?: p.currentPosition
            val dur = p.duration.takeIf { it > 0 } ?: Long.MAX_VALUE
            val target = (base + step).coerceIn(0, dur)
            ui.seekTarget = target
            p.seekTo(target)
            ui.seekFlash++
            ui.bubble(forward, (kotlin.math.abs(step) / 1000).toInt())
        }
    }
    Box(
        Modifier
            .fillMaxSize()
            .pointerInput(Unit) {
                detectTapGestures(
                    onDoubleTap = { o -> seekByTap(o.x > size.width / 2) },
                    onTap = { o ->
                        val forward = o.x > size.width / 2
                        when {
                            ui.seekBubbleActive(forward) -> seekByTap(forward)
                            ui.visible -> ui.dismiss()
                            else -> ui.poke()
                        }
                    },
                )
            },
    ) {
        SeekBubble(ui)
        AnimatedVisibility(ui.visible, enter = fadeIn(), exit = fadeOut()) {
            LaunchedEffect(Unit) { runCatching { playFocus.requestFocus() } }
            Box(Modifier.fillMaxSize()) {
                Box(
                    Modifier
                        .fillMaxSize()
                        .background(
                            Brush.verticalGradient(
                                0f to Color.Black.copy(alpha = 0.7f),
                                0.25f to Color.Transparent,
                                0.6f to Color.Transparent,
                                1f to Color.Black.copy(alpha = 0.85f),
                            ),
                        ),
                )
                Column(Modifier.fillMaxSize().safeDrawingPadding().padding(horizontal = if (tv) 56.dp else 20.dp, vertical = if (tv) 36.dp else 16.dp)) {
                    // Top: what is playing on the left; picture-in-picture and close on the right.
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Column(Modifier.weight(1f)) {
                            Text(
                                ui.title,
                                style = if (tv) MaterialTheme.typography.titleSmall else MaterialTheme.typography.titleMedium,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                            if (ui.subtitle.isNotEmpty()) {
                                Text(ui.subtitle, style = MaterialTheme.typography.bodySmall, color = Muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                        if (!tv) {
                            onPip?.let { RoundButton(Icons.PictureInPicture, tr("Picture in picture"), it, size = 44) }
                            RoundButton(Icons.Close, tr("Close"), onBack, size = 44)
                        }
                    }
                    Spacer(Modifier.weight(1f))
                    // Center transport (also the D-pad home position).
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(if (tv) 28.dp else 36.dp, Alignment.CenterHorizontally),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        val small = if (tv) 44 else 52
                        // An empty slot when there is none, so play/pause stays in the middle.
                        if (ui.hasPrevious) {
                            RoundButton(Icons.SkipPrevious, tr("Previous episode"), onPrevious, size = small)
                        } else {
                            Spacer(Modifier.size(small.dp))
                        }
                        // No −10 s / +30 s buttons: ←/→ seek on TV, a double tap on the left or
                        // right half of the picture does on touch screens.
                        RoundButton(
                            if (ui.playing) Icons.PauseFilled else Icons.Play,
                            if (ui.playing) tr("Pause") else tr("Play"),
                            {
                                player()?.let { if (it.isPlaying) it.pause() else it.play() }
                                ui.poke()
                            },
                            size = if (tv) 60 else 72,
                            modifier = Modifier.focusRequester(playFocus),
                            emphasized = true,
                            loading = ui.buffering,
                        )
                        if (ui.hasNext) {
                            RoundButton(Icons.SkipNext, tr("Next episode"), onNext, size = small)
                        } else {
                            Spacer(Modifier.size(small.dp))
                        }
                    }
                    Spacer(Modifier.weight(1f))
                    // "Skip intro" / "Next episode": above the seek bar, at the right (where it
                    // floats when the controls hide). Same place on every platform.
                    val upNext = ui.nearEnd
                    val skip = ui.skipLabel
                    if (upNext || skip != null) {
                        Row(Modifier.fillMaxWidth().padding(bottom = 12.dp), horizontalArrangement = Arrangement.End) {
                            if (upNext) {
                                val text = ui.upNextIn?.let { tr("Next episode in {seconds} s", "seconds" to it) } ?: tr("Next episode")
                                PillButton(Icons.SkipNext, text, onNext)
                            } else if (skip != null) {
                                PillButton(Icons.SkipNext, skip, onSkip)
                            }
                        }
                    }
                    // Bottom: seek bar, times, extra actions.
                    SeekBar(position, buffered, duration, onSeek = {
                        player()?.seekTo(it)
                        position = it
                        ui.poke()
                    })
                    Row(Modifier.fillMaxWidth().padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            "${formatTime(position)}  /  ${formatTime(duration)}",
                            style = MaterialTheme.typography.bodySmall,
                            color = Muted,
                        )
                        if (duration > 0) {
                            Text(
                                "  ·  " +
                                    tr(
                                        "ends at {time}",
                                        "time" to
                                            java.text.DateFormat
                                                .getTimeInstance(java.text.DateFormat.SHORT, java.util.Locale.forLanguageTag(I18n.language))
                                                .format(java.util.Date(System.currentTimeMillis() + duration - position)),
                                    ),
                                style = MaterialTheme.typography.bodySmall,
                                color = Subtle,
                            )
                        }
                        Spacer(Modifier.weight(1f))
                        // Only in a room (started from the title's page): nothing while watching alone.
                        if (ui.togetherCode != null) {
                            val n = ui.togetherMembers.people().size
                            PillButton(
                                be.alwyapps.watch.ui.Icons.Group,
                                if (n <= 1) tr("Only you so far") else tr("{count} watching", "count" to n),
                                {
                                    roomDialog = true
                                    ui.poke()
                                },
                            )
                            Spacer(Modifier.width(10.dp))
                        } else if (ui.canWatchTogether) {
                            PillButton(be.alwyapps.watch.ui.Icons.Group, tr("Watch together"), onStartTogether)
                            Spacer(Modifier.width(10.dp))
                        }
                        PillButton(Icons.Subtitles, tr("Audio, subtitles & quality"), {
                            ui.tracksOpen = true
                            ui.poke()
                        })
                    }
                }
            }
        }
        // Controls hidden: "Skip intro" / "Next episode" float bottom right and take the focus (OK).
        val prompt =
            when {
                ui.visible || ui.tracksOpen -> null
                ui.nearEnd -> tr("Next episode")
                else -> ui.skipLabel
            }
        AnimatedVisibility(
            prompt != null,
            Modifier.align(Alignment.BottomEnd).safeDrawingPadding().padding(if (tv) 48.dp else 24.dp),
            enter = fadeIn() + slideInHorizontally { it / 2 },
            exit = fadeOut(),
        ) {
            val focus = remember { FocusRequester() }
            val label = prompt ?: return@AnimatedVisibility
            LaunchedEffect(label) { runCatching { focus.requestFocus() } }
            Box(Modifier.focusRequester(focus)) {
                if (label == tr("Next episode")) {
                    PillButton(Icons.SkipNext, ui.upNextIn?.let { tr("Next episode in {seconds} s", "seconds" to it) } ?: label, onNext)
                } else {
                    PillButton(Icons.SkipNext, label, onSkip)
                }
            }
        }
        // Hidden controls, ←/→: a slim bar with the time, gone 2 s after the last press.
        val flash = ui.seekFlash
        LaunchedEffect(flash) {
            if (flash > 0) {
                delay(2000)
                ui.seekTarget = null
            }
        }
        val seekTo = ui.seekTarget
        if (seekTo != null && !ui.visible) {
            val dur = player()?.duration?.takeIf { it > 0 } ?: 0L
            Column(
                Modifier
                    .align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    .background(Brush.verticalGradient(0f to Color.Transparent, 1f to Color.Black.copy(alpha = 0.7f)))
                    .padding(horizontal = if (tv) 56.dp else 20.dp, vertical = if (tv) 28.dp else 16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    "${formatTime(seekTo)}  /  ${formatTime(dur)}",
                    style = MaterialTheme.typography.bodySmall,
                    color = Color.White,
                )
                Box(
                    Modifier
                        .fillMaxWidth()
                        .height(4.dp)
                        .clip(RoundedCornerShape(2.dp))
                        .background(Color.White.copy(alpha = 0.25f)),
                ) {
                    Box(
                        Modifier
                            .fillMaxWidth(if (dur > 0) (seekTo.toFloat() / dur).coerceIn(0f, 1f) else 0f)
                            .height(4.dp)
                            .background(be.alwyapps.watch.ui.Accent),
                    )
                }
            }
        }
        // What the room just did, for a few seconds, controls shown or not.
        val note = ui.togetherNote
        LaunchedEffect(note) {
            if (note != null) {
                delay(4000)
                if (ui.togetherNote == note) ui.togetherNote = null
            }
        }
        val waiting = ui.togetherWaiting
        AnimatedVisibility(
            !note.isNullOrEmpty() || !waiting.isNullOrEmpty(),
            Modifier.align(Alignment.TopCenter).safeDrawingPadding().padding(top = if (tv) 28.dp else 16.dp),
            enter = fadeIn(),
            exit = fadeOut(),
        ) {
            Row(
                Modifier
                    .clip(RoundedCornerShape(50))
                    .background(Color.Black.copy(alpha = 0.7f))
                    .padding(horizontal = 16.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                androidx.compose.material3.Icon(be.alwyapps.watch.ui.Icons.Group, null, Modifier.size(16.dp), tint = Color.White)
                Text(waiting ?: note ?: "", style = MaterialTheme.typography.bodySmall, color = Color.White)
            }
        }
        // With the controls open, loading is the ring around play/pause instead (no overlap).
        if (ui.buffering && !ui.visible) {
            Box(
                Modifier
                    .align(Alignment.Center)
                    .size(56.dp)
                    .clip(RoundedCornerShape(50))
                    .background(Color.Black.copy(alpha = 0.4f)),
                contentAlignment = Alignment.Center,
            ) {
                androidx.compose.material3.CircularProgressIndicator(Modifier.size(32.dp), color = Color.White, strokeWidth = 3.dp)
            }
        }
        // Open: a tap anywhere outside the panel closes it (and does nothing else).
        if (ui.tracksOpen) {
            Box(
                Modifier
                    .fillMaxSize()
                    .clickable(
                        interactionSource =
                            remember {
                                androidx.compose.foundation.interaction
                                    .MutableInteractionSource()
                            },
                        indication = null,
                    ) {
                        ui.tracksOpen = false
                        ui.poke()
                    },
            )
        }
        AnimatedVisibility(
            ui.tracksOpen,
            Modifier.align(Alignment.CenterEnd),
            enter = slideInHorizontally { it } + fadeIn(),
            exit = slideOutHorizontally { it } + fadeOut(),
        ) {
            val close = {
                ui.tracksOpen = false
                ui.poke()
            }
            val server = ui.serverTracks
            if (server != null) {
                ServerTracksPanel(server, ui.quality, onPicked = onServerTrackPicked, onQuality = onQualityPicked, onClose = close)
            } else {
                TracksPanel(player, ui.quality, onPicked = onTrackPicked, onQuality = onQualityPicked, onClose = close)
            }
        }
    }
}

@Composable
private fun RoundButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
    size: Int,
    modifier: Modifier = Modifier,
    emphasized: Boolean = false,
    /** Loading: a spinning ring around the button. */
    loading: Boolean = false,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    val bg =
        when {
            focused -> Color.White
            emphasized -> Color.White.copy(alpha = 0.22f)
            else -> Color.Transparent
        }
    Box(
        modifier
            .size(size.dp)
            .clip(RoundedCornerShape(50))
            .background(bg)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, label, Modifier.size((size * 0.5f).dp), tint = if (focused) Color.Black else Color.White)
        if (loading) {
            androidx.compose.material3.CircularProgressIndicator(
                Modifier.fillMaxSize(),
                color = if (focused) Color.Black.copy(alpha = 0.6f) else Color.White,
                strokeWidth = 3.dp,
            )
        }
    }
}

@Composable
private fun PillButton(
    icon: ImageVector,
    label: String,
    onClick: () -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    Row(
        Modifier
            .clip(RoundedCornerShape(50))
            .background(if (focused) Color.White else Color.White.copy(alpha = 0.16f))
            .clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = 16.dp, vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        val fg = if (focused) Color.Black else Color.White
        Icon(icon, null, Modifier.size(18.dp), tint = fg)
        Text(label, style = MaterialTheme.typography.labelMedium, color = fg)
    }
}

/**
 * Thin seek bar: tap/drag on touch screens; on TV it takes focus and ←/→ move by 10 s
 * (holding the key accelerates).
 */
@Composable
private fun SeekBar(
    position: Long,
    buffered: Long,
    duration: Long,
    onSeek: (Long) -> Unit,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    var width by remember { mutableFloatStateOf(1f) }
    var repeat by remember { mutableIntStateOf(0) }
    val frac = if (duration > 0) (position.toFloat() / duration).coerceIn(0f, 1f) else 0f
    val buf = if (duration > 0) (buffered.toFloat() / duration).coerceIn(0f, 1f) else 0f
    Box(
        Modifier
            .fillMaxWidth()
            .height(28.dp)
            .onSizeChanged { width = it.width.toFloat().coerceAtLeast(1f) }
            .pointerInput(duration) {
                detectTapGestures { o -> if (duration > 0) onSeek((o.x / width * duration).toLong()) }
            }.onKeyEvent {
                if (it.type == KeyEventType.KeyUp) {
                    repeat = 0
                    return@onKeyEvent false
                }
                val step = 10_000L * (1 + repeat / 8)
                when (it.key) {
                    Key.DirectionRight -> {
                        repeat++
                        onSeek((position + step).coerceAtMost(duration))
                        true
                    }
                    Key.DirectionLeft -> {
                        repeat++
                        onSeek((position - step).coerceAtLeast(0))
                        true
                    }
                    else -> false
                }
            }.focusable(interactionSource = interaction),
        contentAlignment = Alignment.CenterStart,
    ) {
        val h = if (focused) 8.dp else 5.dp
        Box(
            Modifier
                .fillMaxWidth()
                .height(h)
                .clip(RoundedCornerShape(50))
                .background(Color.White.copy(alpha = 0.2f)),
        )
        Box(
            Modifier
                .fillMaxWidth(buf)
                .height(h)
                .clip(RoundedCornerShape(50))
                .background(Color.White.copy(alpha = 0.3f)),
        )
        Box(
            Modifier
                .fillMaxWidth(frac)
                .height(h)
                .clip(RoundedCornerShape(50))
                .background(Accent),
        )
        // Thumb.
        Box(
            Modifier
                .fillMaxWidth(frac)
                .height(28.dp),
            contentAlignment = Alignment.CenterEnd,
        ) {
            Box(
                Modifier
                    .size(if (focused) 20.dp else 14.dp)
                    .clip(RoundedCornerShape(50))
                    .background(Color.White)
                    .then(if (focused) Modifier.border(3.dp, Accent, RoundedCornerShape(50)) else Modifier),
            )
        }
    }
}

/** Audio and subtitle tracks of the current media, selectable. */
@Composable
private fun TracksPanel(
    player: () -> Player?,
    quality: QualityInfo?,
    onPicked: (Int, androidx.media3.common.Format?) -> Unit,
    onQuality: (be.alwyapps.watch.core.media.Quality?) -> Unit,
    onClose: () -> Unit,
) {
    val p = player() ?: return
    var version by remember { mutableIntStateOf(0) }
    // The player applies a selection asynchronously: redraw when its tracks actually change
    // (reading them right after a pick showed the previous one).
    androidx.compose.runtime.DisposableEffect(p) {
        val l =
            object : Player.Listener {
                override fun onTracksChanged(t: androidx.media3.common.Tracks) {
                    version++
                }
            }
        p.addListener(l)
        onDispose { p.removeListener(l) }
    }
    val tracks = remember(version) { p.currentTracks }
    val firstFocus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { firstFocus.requestFocus() } }

    fun select(
        type: Int,
        group: androidx.media3.common.Tracks.Group?,
        index: Int,
    ) {
        val b = p.trackSelectionParameters.buildUpon()
        if (group == null) {
            b.setTrackTypeDisabled(type, true)
        } else {
            b.setTrackTypeDisabled(type, false).setOverrideForType(TrackSelectionOverride(group.mediaTrackGroup, index))
        }
        p.trackSelectionParameters = b.build()
        version++
        onPicked(type, group?.getTrackFormat(index))
    }

    data class Option(
        val label: String,
        val selected: Boolean,
        val pick: () -> Unit,
    )

    fun options(type: Int): List<Option> {
        val out = mutableListOf<Option>()
        for (g in tracks.groups.filter { it.type == type }) {
            for (i in 0 until g.length) {
                if (!g.isTrackSupported(i)) continue
                val f = g.getTrackFormat(i)
                val label =
                    listOfNotNull(
                        f.label ?: f.language?.let {
                            java.util.Locale
                                .forLanguageTag(it)
                                .getDisplayLanguage(java.util.Locale.forLanguageTag(I18n.language))
                                .ifEmpty { it }
                                .replaceFirstChar { c -> c.uppercase() }
                        },
                        if (type == C.TRACK_TYPE_AUDIO && f.channelCount > 0) {
                            when (f.channelCount) {
                                1 -> tr("Mono")
                                2 -> tr("Stereo")
                                6 -> "5.1"
                                8 -> "7.1"
                                else -> tr("{count} ch", "count" to f.channelCount)
                            }
                        } else {
                            null
                        },
                        // Media3 converts subtitles to "x-media3-cues" while extracting; the original format is in codecs.
                        friendlyFormat(if (f.sampleMimeType == "application/x-media3-cues") f.codecs else f.sampleMimeType ?: f.codecs),
                    ).joinToString(" · ").ifEmpty { tr("Track {number}", "number" to i + 1) }
                out += Option(label, g.isTrackSelected(i)) { select(type, g, i) }
            }
        }
        return out
    }
    val audio = options(C.TRACK_TYPE_AUDIO)
    val subs = options(C.TRACK_TYPE_TEXT)
    val subsOff = p.trackSelectionParameters.disabledTrackTypes.contains(C.TRACK_TYPE_TEXT) || subs.none { it.selected }
    Column(
        Modifier
            .fillMaxHeight()
            .width(if (LocalDimens.current.isTv) 340.dp else 320.dp)
            .background(Color(0xF2111116))
            // Taps on the panel itself stay here (the layer behind closes it).
            .pointerInput(Unit) { detectTapGestures { } }
            .safeDrawingPadding()
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(tr("Audio, subtitles & quality"), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
            RoundButton(Icons.Close, tr("Close"), onClose, size = 36)
        }
        LazyColumn(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            item { Section(tr("Audio")) }
            items(audio.withIndex().toList()) { (i, o) -> TrackRow(o.label, o.selected, o.pick, if (i == 0) Modifier.focusRequester(firstFocus) else Modifier) }
            item { Section(tr("Subtitles")) }
            item { TrackRow(tr("Off"), subsOff, { select(C.TRACK_TYPE_TEXT, null, 0) }) }
            items(subs) { o -> TrackRow(o.label, o.selected && !subsOff, o.pick) }
            qualityItems(quality, onQuality)
        }
    }
}

/** The conversion's choice among the file's streams; a pick restarts it with the new tracks. */
@Composable
private fun ServerTracksPanel(
    tracks: ServerTracks,
    quality: QualityInfo?,
    onPicked: (be.alwyapps.watch.core.media.StreamKind, Long?) -> Unit,
    onQuality: (be.alwyapps.watch.core.media.Quality?) -> Unit,
    onClose: () -> Unit,
) {
    val firstFocus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { firstFocus.requestFocus() } }
    val audio = tracks.streams.filter { it.kind == be.alwyapps.watch.core.media.StreamKind.AUDIO }
    val subs = tracks.streams.filter { it.kind == be.alwyapps.watch.core.media.StreamKind.SUBTITLE }
    val sel = tracks.selection
    // Nothing chosen for the audio: the server's default stream plays.
    val audioNow = sel.audioIndex ?: audio.firstOrNull { it.isDefault }?.index ?: audio.firstOrNull()?.index
    val subNow = (sel.subtitle as? be.alwyapps.watch.core.media.SubtitleSelection.Stream)?.index

    fun label(s: be.alwyapps.watch.core.media.MediaStream): String =
        s.displayTitle ?: listOfNotNull(s.title ?: languageName(s.language), s.codec.uppercase().ifEmpty { null }).joinToString(" · ")
    Column(
        Modifier
            .fillMaxHeight()
            .width(if (LocalDimens.current.isTv) 340.dp else 320.dp)
            .background(Color(0xF2111116))
            .pointerInput(Unit) { detectTapGestures { } }
            .safeDrawingPadding()
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(tr("Audio, subtitles & quality"), Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
            RoundButton(Icons.Close, tr("Close"), onClose, size = 36)
        }
        Text(tr("Converted by the server: changing a track restarts the stream"), style = MaterialTheme.typography.labelSmall, color = Subtle)
        LazyColumn(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            item { Section(tr("Audio")) }
            items(audio.withIndex().toList()) { (i, s) ->
                TrackRow(
                    label(s),
                    s.index == audioNow,
                    { if (s.index != audioNow) onPicked(s.kind, s.index) },
                    if (i == 0) Modifier.focusRequester(firstFocus) else Modifier,
                )
            }
            item { Section(tr("Subtitles")) }
            item { TrackRow(tr("Off"), subNow == null, { if (subNow != null) onPicked(be.alwyapps.watch.core.media.StreamKind.SUBTITLE, null) }) }
            items(subs) { s -> TrackRow(label(s), s.index == subNow, { if (s.index != subNow) onPicked(s.kind, s.index) }) }
            qualityItems(quality, onQuality)
        }
    }
}

/** Quality: what plays now, then the original and the sizes the server converts to. */
private fun androidx.compose.foundation.lazy.LazyListScope.qualityItems(
    quality: QualityInfo?,
    onQuality: (be.alwyapps.watch.core.media.Quality?) -> Unit,
) {
    quality ?: return
    item { Section(tr("Quality")) }
    item {
        Text(
            tr("Now: {quality}", "quality" to quality.label),
            Modifier.padding(horizontal = 12.dp, vertical = 2.dp),
            style = MaterialTheme.typography.labelSmall,
            color = Muted,
        )
    }
    items(quality.choices) { c -> TrackRow(c.label, c.quality == quality.chosen, { onQuality(c.quality) }) }
}

@Composable
private fun Section(title: String) {
    Text(title.uppercase(), Modifier.padding(top = 14.dp, bottom = 4.dp), style = MaterialTheme.typography.labelSmall, color = Subtle)
}

@Composable
private fun TrackRow(
    label: String,
    selected: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val interaction = remember { MutableInteractionSource() }
    val focused by interaction.collectIsFocusedAsState()
    Row(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(12.dp))
            .background(if (focused) Color.White else Color.Transparent)
            .clickable(interactionSource = interaction, indication = null, onClick = onClick)
            .padding(horizontal = 12.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val fg = if (focused) Color.Black else Color.White
        Text(label, Modifier.weight(1f), color = fg, style = MaterialTheme.typography.bodySmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (selected) Icon(Icons.Check, tr("Selected"), Modifier.size(18.dp), tint = if (focused) Color.Black else Accent)
    }
}

/** "Dolby Digital+", "TrueHD", "SRT"… instead of MIME types. */
private fun friendlyFormat(mime: String?): String? {
    val m = mime?.lowercase() ?: return null
    return when {
        "eac3-joc" in m -> "Dolby Atmos"
        "eac3" in m || "ec-3" in m -> "Dolby Digital+"
        "ac3" in m || "ac-3" in m -> "Dolby Digital"
        "truehd" in m -> "TrueHD"
        "dts" in m -> if ("hd" in m) "DTS-HD" else "DTS"
        "mp4a" in m || "aac" in m -> "AAC"
        "opus" in m -> "Opus"
        "flac" in m -> "FLAC"
        "vorbis" in m -> "Vorbis"
        "mpeg" in m && "audio" in m -> "MP3"
        "subrip" in m -> "SRT"
        "ssa" in m || "ass" in m -> "ASS"
        "pgs" in m -> "PGS"
        "vtt" in m -> "WebVTT"
        "ttml" in m -> "TTML"
        "dvb" in m -> "DVB"
        "vobsub" in m || "dvd" in m -> "VobSub"
        "raw" in m -> null
        else -> m.substringAfterLast('/').uppercase()
    }
}

/** How long a seek bubble stays, and presses on its side keep adding to it. */
private const val SEEK_BUBBLE_MS = 900L

/**
 * "+30" / "−10" on the side seeked to, under an arrow that turns once per press, fading
 * quickly; presses that follow each other add up (+30, +60…).
 */
@Composable
private fun SeekBubble(ui: PlayerUi) {
    val (forward, seconds) = ui.seekBubble ?: return
    var shown by remember { mutableStateOf(false) }
    val spin =
        remember {
            androidx.compose.animation.core
                .Animatable(0f)
        }
    LaunchedEffect(ui.seekBubbleN) {
        if (ui.seekBubbleN == 0) return@LaunchedEffect
        shown = true
        launch {
            spin.snapTo(0f)
            spin.animateTo(
                if (forward) 360f else -360f,
                androidx.compose.animation.core
                    .tween(450),
            )
        }
        delay(SEEK_BUBBLE_MS - 250)
        shown = false
    }
    val alpha by androidx.compose.animation.core.animateFloatAsState(
        if (shown) 1f else 0f,
        androidx.compose.animation.core
            .tween(if (shown) 90 else 250),
        label = "seek-bubble",
    )
    val tv = LocalDimens.current.isTv
    Box(Modifier.fillMaxSize().graphicsLayer { this.alpha = alpha }) {
        Column(
            Modifier
                .align(if (forward) Alignment.CenterEnd else Alignment.CenterStart)
                .padding(horizontal = if (tv) 120.dp else 48.dp)
                .size(if (tv) 112.dp else 96.dp)
                .background(Color.Black.copy(alpha = 0.45f), androidx.compose.foundation.shape.CircleShape),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.Center,
        ) {
            Icon(
                Icons.Refresh,
                null,
                Modifier
                    .size(if (tv) 40.dp else 34.dp)
                    // The arrow turns clockwise forward; mirrored, it turns back.
                    .graphicsLayer {
                        rotationZ = spin.value
                        scaleX = if (forward) 1f else -1f
                    },
                tint = Color.White,
            )
            Text(
                if (forward) "+$seconds" else "−$seconds",
                style = MaterialTheme.typography.titleMedium,
                color = Color.White,
            )
        }
    }
}
