// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.player

import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.view.KeyEvent
import android.view.WindowManager
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.annotation.OptIn
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import androidx.lifecycle.lifecycleScope
import androidx.media3.common.AudioAttributes
import androidx.media3.common.C
import androidx.media3.common.MediaItem
import androidx.media3.common.MimeTypes
import androidx.media3.common.PlaybackException
import androidx.media3.common.Player
import androidx.media3.common.Tracks
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DefaultDataSource
import androidx.media3.datasource.DefaultHttpDataSource
import androidx.media3.exoplayer.DefaultRenderersFactory
import androidx.media3.exoplayer.ExoPlayer
import androidx.media3.exoplayer.trackselection.DefaultTrackSelector
import androidx.media3.ui.PlayerView
import be.alwyapps.watch.BuildConfig
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.CoreException
import be.alwyapps.watch.core.media.HttpRequest
import be.alwyapps.watch.core.media.ItemKind
import be.alwyapps.watch.core.media.MediaSource
import be.alwyapps.watch.core.media.PlaybackEvent
import be.alwyapps.watch.core.media.PlaybackInfo
import be.alwyapps.watch.core.media.ProgressReport
import be.alwyapps.watch.core.media.Segment
import be.alwyapps.watch.core.media.SegmentKind
import be.alwyapps.watch.core.media.StreamKind
import be.alwyapps.watch.services
import io.github.peerless2012.ass.media.kt.buildWithAssSupport
import io.github.peerless2012.ass.media.type.AssRenderType
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * Full-screen Media3 player. Direct play of the original file whenever the device can decode it
 * (hardware decoders first, FFmpeg audio as fallback, passthrough negotiated by ExoPlayer),
 * server transcoding only as a last resort. Reports progress to the server every 10 s.
 */
@OptIn(UnstableApi::class)
class PlayerActivity : ComponentActivity() {
    private var player: ExoPlayer? = null
    private lateinit var view: PlayerView
    private lateinit var core: AlwyWatch
    private lateinit var account: String
    private lateinit var itemId: String
    private var info: PlaybackInfo? = null
    private var source: MediaSource? = null
    private var progressJob: Job? = null
    private var nextEpisodeId: String? = null
    private var previousEpisodeId: String? = null
    private var nextStartMs = 0L
    private var previousStartMs = 0L
    private var segments: List<Segment> = emptyList()
    private var segmentJob: Job? = null
    private var autoSkipIntro = false
    private var autoPlayNext = true

    /** Segments already skipped automatically (by start): seeking back into one plays it. */
    private val autoSkipped = mutableSetOf<ULong>()

    /** Uptime the credits countdown started, or null; cancelled for this episode by Back. */
    private var countdownFrom: Long? = null
    private var countdownCancelled = false

    /** The Back press that went down was used (closing a panel, hiding the controls): eat its release too. */
    private var backUsed = false

    /** Event time of the last Back press (to drop remotes' duplicate events). */
    private var lastBackAt = Long.MIN_VALUE / 2
    private var prefetchedNext = false
    private var originalModeId = 0
    private var transcoding = false
    private val ui = PlayerUi()

    // ----- watch together: the room this player follows (null: alone) -----
    private var room: String? = null
    private var member: String? = null
    private var roomSeq = 0uL
    private var roomMedia: be.alwyapps.watch.core.keep.TogetherMedia? = null
    private var roomState: be.alwyapps.watch.core.keep.TogetherState? = null
    private var roomReceivedAt = 0L
    private var roomJob: Job? = null

    /** Applying the room's state (or starting a title): the player's events are not ours to send. */
    private var applyingRoom = false

    /** Our own updates on their way: the room answers in between are older than them. */
    private var roomSending = 0

    /** This title was chosen here (next episode…): tell the room once it starts. */
    private var announceTitle = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        account = intent.getStringExtra(EXTRA_ACCOUNT) ?: return finish()
        itemId = intent.getStringExtra(EXTRA_ITEM) ?: return finish()
        room = intent.getStringExtra(EXTRA_ROOM)
        member = intent.getStringExtra(EXTRA_MEMBER)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowCompat.getInsetsController(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }
        originalModeId = window.attributes.preferredDisplayModeId
        view =
            PlayerView(this).apply {
                // Our own Compose controls draw on top; PlayerView keeps the surface and the
                // subtitle view (libass renders into it).
                useController = false
                keepScreenOn = true
            }
        val overlay =
            androidx.compose.ui.platform.ComposeView(this).apply {
                setContent {
                    be.alwyapps.watch.ui.AlwyTheme(isTv = services.isTv) {
                        PlayerOverlay(
                            ui,
                            { player },
                            onBack = { finish() },
                            onNext = { playNext() },
                            onPrevious = { playPrevious() },
                            onSkip = { skipSegment() },
                            onPip = if (pipSupported) ({ enterPip() }) else null,
                            onTogether = { startTogether() },
                            onLeaveTogether = { leaveTogether("You left watch together") },
                        )
                    }
                }
            }
        setContentView(
            android.widget.FrameLayout(this).apply {
                setBackgroundColor(android.graphics.Color.BLACK)
                addView(view)
                addView(overlay)
            },
        )
        val start = intent.getLongExtra(EXTRA_START, 0L)
        lifecycleScope.launch {
            core = services.core.await()
            ui.togetherAvailable = runCatching { core.keepActive() }.getOrDefault(false)
            load(itemId, start)
            if (room != null) followRoom()
        }
    }

    private suspend fun load(
        id: String,
        startMs: Long,
    ) {
        itemId = id
        showTitle(id)
        findAdjacent(id)
        loadSegments(id)
        // Downloaded: play the local original file (works offline; progress is queued).
        core.downloadedFile(account, id)?.let { path ->
            val local =
                HttpRequest(
                    url =
                        android.net.Uri
                            .fromFile(java.io.File(path))
                            .toString(),
                    headers = emptyList(),
                )
            info = null
            source =
                be.alwyapps.watch.core.media.MediaSource(
                    id = id,
                    container = null,
                    size = null,
                    bitrate = null,
                    durationMs = null,
                    fileName = null,
                    streams = emptyList(),
                    direct = local,
                    transcode = null,
                    supportsDirectPlay = true,
                    defaultAudioIndex = null,
                    defaultSubtitleIndex = null,
                )
            startPlayer(local, source!!, transcoding = false, startMs)
            return
        }
        val profile = DeviceProfiles.get(this)
        val pi =
            try {
                core.playbackInfo(account, id, profile)
            } catch (e: CoreException) {
                Toast.makeText(this, e.message, Toast.LENGTH_LONG).show()
                finish()
                return
            }
        val src = pi.sources.firstOrNull()
        if (src == null) {
            Toast.makeText(this, "Nothing to play", Toast.LENGTH_LONG).show()
            finish()
            return
        }
        info = pi
        source = src
        val direct = DeviceProfiles.canDirectPlay(src, profile)
        val request = if (direct || src.transcode == null) src.direct else src.transcode!!
        startPlayer(request, src, transcoding = !direct && src.transcode != null, startMs)
    }

    private fun startPlayer(
        req: HttpRequest,
        src: MediaSource,
        transcoding: Boolean,
        startMs: Long,
    ) {
        player?.release()
        this.transcoding = transcoding
        val settings = PlayerSettings(core, services.isTv)
        val http =
            DefaultHttpDataSource
                .Factory()
                .setUserAgent("AlwyWatch/${BuildConfig.VERSION_NAME}")
                .setAllowCrossProtocolRedirects(true)
                .setDefaultRequestProperties(req.headers.associate { it.name to it.value })
        val dataSource = DefaultDataSource.Factory(this, http)
        val renderers =
            DefaultRenderersFactory(this)
                // Platform (hardware) decoders first; FFmpeg only when no platform decoder exists.
                .setExtensionRendererMode(DefaultRenderersFactory.EXTENSION_RENDERER_MODE_ON)
                .setEnableDecoderFallback(true)
        val selector = DefaultTrackSelector(this)
        selector.parameters =
            selector
                .buildUponParameters()
                .setTunnelingEnabled(settings.tunneling)
                .setPreferredTextLanguage(null)
                .build()
        val p =
            ExoPlayer
                .Builder(this)
                .setTrackSelector(selector)
                .setAudioAttributes(
                    AudioAttributes
                        .Builder()
                        .setUsage(C.USAGE_MEDIA)
                        .setContentType(C.AUDIO_CONTENT_TYPE_MOVIE)
                        .build(),
                    true,
                ).setHandleAudioBecomingNoisy(true)
                .setSeekBackIncrementMs(10_000)
                .setSeekForwardIncrementMs(30_000)
                .buildWithAssSupport(
                    this,
                    AssRenderType.OVERLAY_OPEN_GL,
                    subtitleView = view.subtitleView,
                    dataSourceFactory = dataSource,
                    renderersFactory = renderers,
                )
        if (settings.matchFrameRate) {
            p.videoChangeFrameRateStrategy = C.VIDEO_CHANGE_FRAME_RATE_STRATEGY_ONLY_IF_SEAMLESS
        }
        val item =
            MediaItem
                .Builder()
                .setUri(req.url)
                .setMediaId(itemId)
                .apply { if (transcoding) setMimeType(MimeTypes.APPLICATION_M3U8) }
                .setSubtitleConfigurations(
                    src.streams
                        .filter { it.kind == StreamKind.SUBTITLE && it.external != null }
                        .mapNotNull { s ->
                            val mime =
                                when (s.codec) {
                                    "srt", "subrip" -> MimeTypes.APPLICATION_SUBRIP
                                    "ass", "ssa" -> MimeTypes.TEXT_SSA
                                    "vtt", "webvtt" -> MimeTypes.TEXT_VTT
                                    "ttml" -> MimeTypes.APPLICATION_TTML
                                    else -> return@mapNotNull null
                                }
                            MediaItem.SubtitleConfiguration
                                .Builder(android.net.Uri.parse(s.external!!.url))
                                .setMimeType(mime)
                                .setLanguage(s.language)
                                .setLabel(s.displayTitle ?: s.title)
                                .setSelectionFlags(
                                    (if (s.isDefault) C.SELECTION_FLAG_DEFAULT else 0) or (if (s.isForced) C.SELECTION_FLAG_FORCED else 0),
                                ).build()
                        },
                ).build()
        p.addListener(listener)
        applyingRoom = true
        p.setMediaItem(item, startMs)
        p.prepare()
        p.playWhenReady = true
        applyingRoom = false
        view.player = p
        player = p
        // In a room: a title chosen here is announced, one chosen by the room follows its state.
        if (room != null) {
            if (announceTitle) {
                announceTitle = false
                sendRoom(newTitle = true)
            } else {
                roomState?.let { syncToRoom(it) }
            }
        }
        report(PlaybackEvent.START)
        progressJob?.cancel()
        progressJob =
            lifecycleScope.launch {
                while (isActive) {
                    delay(10_000)
                    val pl = player ?: break
                    if (pl.isPlaying) report(PlaybackEvent.PROGRESS)
                    maybePrefetchNext(pl)
                }
            }
        segmentJob?.cancel()
        segmentJob =
            lifecycleScope.launch {
                while (isActive) {
                    player?.let { watchSegments(it) }
                    delay(500)
                }
            }
    }

    private val listener =
        object : Player.Listener {
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                ui.playing = isPlaying
                if (!isPlaying) ui.poke()
                val p = player ?: return
                if (!isPlaying && p.playbackState == Player.STATE_READY) report(PlaybackEvent.PAUSE)
            }

            override fun onPlayWhenReadyChanged(
                playWhenReady: Boolean,
                reason: Int,
            ) {
                if (!applyingRoom && reason == Player.PLAY_WHEN_READY_CHANGE_REASON_USER_REQUEST) sendRoom()
            }

            override fun onPositionDiscontinuity(
                oldPosition: Player.PositionInfo,
                newPosition: Player.PositionInfo,
                reason: Int,
            ) {
                if (!applyingRoom && reason == Player.DISCONTINUITY_REASON_SEEK) sendRoom()
            }

            override fun onPlaybackStateChanged(state: Int) {
                ui.buffering = state == Player.STATE_BUFFERING || state == Player.STATE_IDLE
                if (state == Player.STATE_ENDED) {
                    report(PlaybackEvent.STOP)
                    val next = nextEpisodeId
                    android.util.Log.i(TAG, "ended; next episode: ${next != null}")
                    if (next != null) {
                        announceTitle = room != null
                        lifecycleScope.launch { load(next, freshStart(next, nextStartMs)) }
                    } else if (room == null) {
                        finish()
                    }
                }
            }

            override fun onPlayerError(error: PlaybackException) {
                val src = source ?: return
                val fallback = src.transcode
                val decoderProblem =
                    error.errorCode in
                        setOf(
                            PlaybackException.ERROR_CODE_DECODER_INIT_FAILED,
                            PlaybackException.ERROR_CODE_DECODER_QUERY_FAILED,
                            PlaybackException.ERROR_CODE_DECODING_FAILED,
                            PlaybackException.ERROR_CODE_DECODING_FORMAT_EXCEEDS_CAPABILITIES,
                            PlaybackException.ERROR_CODE_DECODING_FORMAT_UNSUPPORTED,
                        )
                if (!transcoding && fallback != null && decoderProblem) {
                    // Direct play first; the server's conversion only when this device fails.
                    val position = player?.currentPosition ?: 0L
                    Toast.makeText(this@PlayerActivity, "This device can't decode this file: using the server's converted version", Toast.LENGTH_LONG).show()
                    lifecycleScope.launch { startPlayer(fallback, src, transcoding = true, position) }
                } else {
                    Toast.makeText(this@PlayerActivity, "Playback failed: ${error.errorCodeName}", Toast.LENGTH_LONG).show()
                }
            }

            override fun onTracksChanged(tracks: Tracks) {
                if (!PlayerSettings(core, services.isTv).matchFrameRate) return
                val fmt =
                    tracks.groups
                        .firstOrNull { it.type == C.TRACK_TYPE_VIDEO && it.isSelected }
                        ?.let { g -> (0 until g.length).firstOrNull { g.isTrackSelected(it) }?.let { g.getTrackFormat(it) } }
                        ?: return
                if (fmt.frameRate > 0) switchDisplayMode(fmt.frameRate, fmt.width, fmt.height)
            }
        }

    /** Picks the display mode with the same resolution whose refresh rate is a whole multiple of the video. */
    @Suppress("DEPRECATION")
    private fun switchDisplayMode(
        fps: Float,
        width: Int,
        height: Int,
    ) {
        val display = windowManager.defaultDisplay ?: return
        val current = display.mode
        val candidates =
            display.supportedModes.filter {
                it.physicalWidth == current.physicalWidth &&
                    it.physicalHeight == current.physicalHeight
            }
        val best =
            candidates.minByOrNull { m ->
                val ratio = m.refreshRate / fps
                val err = abs(ratio - ratio.roundToInt())
                if (ratio.roundToInt() < 1) Float.MAX_VALUE else err + m.refreshRate / 1000f
            } ?: return
        val ratio = best.refreshRate / fps
        if (abs(ratio - ratio.roundToInt()) > 0.01f || best.modeId == current.modeId) return
        window.attributes = window.attributes.apply { preferredDisplayModeId = best.modeId }
    }

    /** Previous and next episodes (across seasons), for the buttons and auto-play. */
    private fun findAdjacent(id: String) {
        nextEpisodeId = null
        previousEpisodeId = null
        prefetchedNext = false
        ui.hasNext = false
        ui.hasPrevious = false
        lifecycleScope.launch {
            val adj =
                runCatching { core.adjacentEpisodes(account, id) }
                    .onFailure { android.util.Log.w(TAG, "adjacent episodes: ${it.javaClass.simpleName}") }
                    .getOrNull() ?: return@launch
            if (itemId != id) return@launch
            nextEpisodeId = adj.next?.id
            previousEpisodeId = adj.previous?.id
            nextStartMs = adj.next?.let(::resumeAt) ?: 0L
            previousStartMs = adj.previous?.let(::resumeAt) ?: 0L
            ui.hasNext = nextEpisodeId != null
            ui.hasPrevious = previousEpisodeId != null
        }
    }

    /** Where an episode was left (servers clear the position when it is finished), like the Resume button. */
    private fun resumeAt(item: be.alwyapps.watch.core.media.Item): Long = item.user.positionMs.toLong()

    /** The episode's current resume position from the server (1.5 s at most), else the cached one. */
    private suspend fun freshStart(
        id: String,
        cached: Long,
    ): Long =
        kotlinx.coroutines.withTimeoutOrNull(1500) {
            runCatching { core.refreshItem(account, id, true).item }.getOrNull()?.let(::resumeAt)
        } ?: cached

    private fun loadSegments(id: String) {
        segments = emptyList()
        ui.skipLabel = null
        ui.upNextIn = null
        autoSkipped.clear()
        countdownFrom = null
        countdownCancelled = false
        PlayerSettings(core, services.isTv).let {
            autoSkipIntro = it.autoSkipIntro
            autoPlayNext = it.autoPlayNext
        }
        lifecycleScope.launch {
            val list = runCatching { core.segments(account, id) }.getOrNull() ?: return@launch
            android.util.Log.i(TAG, "segments: " + list.joinToString { "${it.kind} ${it.startMs / 1000u}-${it.endMs / 1000u}s" })
            if (itemId == id) segments = list
        }
    }

    private fun prompt(p: Player) =
        be.alwyapps.watch.core.playbackPrompt(
            segments,
            p.currentPosition.coerceAtLeast(0).toULong(),
            p.duration.takeIf { it > 0 }?.toULong(),
            nextEpisodeId != null,
        )

    /** Skip intro/credits prompt, and "Next episode" during the credits (or the last 40 s). */
    private fun watchSegments(p: Player) {
        val prompt = prompt(p)
        val seg = prompt.skip
        ui.nearEnd = prompt.nextEpisode
        if (seg != null && autoSkipIntro && seg.kind in AUTO_SKIP && autoSkipped.add(seg.startMs)) {
            p.seekTo(seg.endMs.toLong())
            ui.skipLabel = null
            return
        }
        ui.skipLabel =
            when (seg?.kind) {
                null -> null
                SegmentKind.CREDITS -> "Skip credits"
                SegmentKind.INTRO -> "Skip intro"
                SegmentKind.RECAP -> "Skip recap"
                SegmentKind.PREVIEW -> "Skip preview"
                SegmentKind.COMMERCIAL -> "Skip"
            }
        // Credits with a next episode: count down, then play it (unless Back cancelled it).
        if (prompt.credits && autoPlayNext && !countdownCancelled && p.isPlaying) {
            val from =
                countdownFrom ?: android.os.SystemClock
                    .uptimeMillis()
                    .also { countdownFrom = it }
            val left = COUNTDOWN_S - ((android.os.SystemClock.uptimeMillis() - from) / 1000).toInt()
            if (left <= 0) {
                ui.upNextIn = null
                playNext()
                return
            }
            ui.upNextIn = left
        } else {
            countdownFrom = null
            ui.upNextIn = null
        }
    }

    private fun skipSegment() {
        val p = player ?: return
        val seg = prompt(p).skip ?: return
        p.seekTo(seg.endMs.toLong())
        ui.skipLabel = null
    }

    private fun maybePrefetchNext(p: Player) {
        val next = nextEpisodeId ?: return
        if (prefetchedNext || p.duration <= 0) return
        if (p.currentPosition > p.duration * 0.8) {
            prefetchedNext = true
            core.prefetchPlaybackInfo(account, next, DeviceProfiles.get(this))
        }
    }

    private fun report(event: PlaybackEvent): Job? {
        val p = player ?: return null
        val src = source ?: return null
        val report =
            ProgressReport(
                itemId = itemId,
                mediaSourceId = src.id,
                playSessionId = info?.playSessionId,
                positionMs = p.currentPosition.coerceAtLeast(0).toULong(),
                durationMs = p.duration.takeIf { it > 0 }?.toULong(),
                event = event,
                audioIndex = null,
                subtitleIndex = null,
            )
        return services.scope.launch { runCatching { core.reportProgress(account, report) } }
    }

    private fun playNext() {
        val next = nextEpisodeId ?: return
        report(PlaybackEvent.STOP)
        ui.hasNext = false
        announceTitle = room != null
        val start = nextStartMs
        lifecycleScope.launch { load(next, freshStart(next, start)) }
    }

    private fun playPrevious() {
        val prev = previousEpisodeId ?: return
        report(PlaybackEvent.STOP)
        ui.hasPrevious = false
        announceTitle = room != null
        val start = previousStartMs
        lifecycleScope.launch { load(prev, freshStart(prev, start)) }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val down = event.action == KeyEvent.ACTION_DOWN
        if (event.keyCode == KeyEvent.KEYCODE_BACK && down && event.repeatCount == 0) {
            // Some remotes (Chromecast with Google TV) send Back twice per press, ~20 ms apart:
            // the echo must not count as a second press (it would leave right after hiding).
            val echo = event.eventTime - lastBackAt < BACK_ECHO_MS
            lastBackAt = event.eventTime
            if (echo) {
                backUsed = true
                return true
            }
        }
        val p = player
        when (event.keyCode) {
            KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE, KeyEvent.KEYCODE_MEDIA_PLAY, KeyEvent.KEYCODE_MEDIA_PAUSE -> {
                if (down && p != null) {
                    if (event.keyCode == KeyEvent.KEYCODE_MEDIA_PAUSE ||
                        (event.keyCode == KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE && p.isPlaying)
                    ) {
                        p.pause()
                    } else {
                        p.play()
                    }
                    ui.poke()
                }
                return true
            }
            KeyEvent.KEYCODE_MEDIA_FAST_FORWARD -> {
                if (down) p?.seekForward()
                ui.poke()
                return true
            }
            KeyEvent.KEYCODE_MEDIA_REWIND -> {
                if (down) p?.seekBack()
                ui.poke()
                return true
            }
            KeyEvent.KEYCODE_MEDIA_NEXT -> {
                if (down) playNext()
                return true
            }
            KeyEvent.KEYCODE_MEDIA_PREVIOUS -> {
                if (down) playPrevious()
                return true
            }
            KeyEvent.KEYCODE_BACK -> {
                // Close the panel, then hide the controls, then leave. Controls that hid on their
                // own just before the press count as shown: that Back was meant to hide them.
                if (down) {
                    if (event.repeatCount == 0) {
                        backUsed =
                            when {
                                ui.upNextIn != null && !ui.visible -> {
                                    countdownCancelled = true
                                    ui.upNextIn = null
                                    true
                                }
                                ui.tracksOpen -> {
                                    ui.tracksOpen = false
                                    true
                                }
                                ui.visible -> {
                                    ui.dismiss()
                                    true
                                }
                                else -> android.os.SystemClock.uptimeMillis() - ui.hiddenAt < BACK_GRACE_MS
                            }
                    }
                    if (backUsed) return true
                } else if (backUsed) {
                    backUsed = false
                    return true
                }
            }
        }
        if (!ui.visible && (ui.nearEnd || ui.skipLabel != null) && (event.keyCode == KeyEvent.KEYCODE_DPAD_CENTER || event.keyCode == KeyEvent.KEYCODE_ENTER)) {
            // The "Next episode" / "Skip intro" prompt has the focus: OK goes to it.
            return super.dispatchKeyEvent(event)
        }
        if (!ui.visible) {
            // Hidden: ←/→ seek right away (like every TV player), other keys wake the controls.
            when (event.keyCode) {
                KeyEvent.KEYCODE_DPAD_LEFT, KeyEvent.KEYCODE_DPAD_RIGHT -> {
                    if (down && p != null) {
                        if (event.keyCode == KeyEvent.KEYCODE_DPAD_LEFT) p.seekBack() else p.seekForward()
                    }
                    ui.poke()
                    return true
                }
                KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER -> {
                    if (down) ui.poke()
                    return true
                }
            }
        } else if (down) {
            ui.touch++
        }
        return super.dispatchKeyEvent(event)
    }

    // ----- watch together ------------------------------------------------------------------

    /** Opens a room with what plays now; its code shows in the controls for friends to join. */
    private fun startTogether() {
        if (room != null) return
        val p = player ?: return
        ui.togetherNote = "Opening a room\u2026"
        lifecycleScope.launch {
            try {
                val joined = core.togetherCreate(account, itemId, p.currentPosition.coerceAtLeast(0).toULong(), p.playWhenReady)
                room = joined.state.code
                member = joined.memberId
                acceptRoom(joined.state)
                roomMedia = joined.state.media
                ui.togetherNote = "Room ${spaced(joined.state.code)}: share this code"
                followRoom()
            } catch (e: CoreException) {
                ui.togetherNote = "Watch together unavailable: ${e.message}"
            }
        }
    }

    private fun spaced(code: String) = if (code.length == 6) code.substring(0, 3) + " " + code.substring(3) else code

    private fun acceptRoom(s: be.alwyapps.watch.core.keep.TogetherState) {
        roomState = s
        roomReceivedAt = android.os.SystemClock.elapsedRealtime()
        if (s.seq > roomSeq) roomSeq = s.seq
        ui.togetherCode = spaced(s.code)
        ui.togetherPeople = s.members.map { it.name }
    }

    /** Long-polls the room and applies what the others do. */
    private fun followRoom() {
        roomJob?.cancel()
        roomJob =
            lifecycleScope.launch {
                var failures = 0
                while (isActive) {
                    val code = room ?: break
                    val me = member ?: break
                    val s =
                        try {
                            core.togetherWait(code, me, roomSeq)
                        } catch (e: CoreException) {
                            if (e is CoreException.NotFound) {
                                leaveTogether("The watch together room closed", tellServer = false)
                                break
                            }
                            failures++
                            delay((failures * 2000L).coerceAtMost(10_000))
                            continue
                        }
                    failures = 0
                    if (room != code) break
                    onRoomState(s)
                }
            }
    }

    private fun onRoomState(s: be.alwyapps.watch.core.keep.TogetherState) {
        val before = roomState
        acceptRoom(s)
        if (s.by == member || roomSending > 0) {
            roomMedia = s.media
            return
        }
        val known = roomMedia
        roomMedia = s.media
        if (known != null && known != s.media) {
            followTitle(s)
            return
        }
        // Someone else played, paused or moved: say who, then follow.
        if (before != null && s.seq != before.seq) {
            val who = s.members.firstOrNull { it.id == s.by }?.name
            if (who != null) {
                ui.togetherNote =
                    when {
                        before.playing && !s.playing -> "$who paused"
                        !before.playing && s.playing -> "$who resumed"
                        else -> "$who moved to ${formatTime(s.positionMs.toLong())}"
                    }
            } else if (s.members.size != before.members.size) {
                ui.togetherNote = "${s.members.size} watching"
            }
        }
        syncToRoom(s)
    }

    /** Plays or pauses like the room and seeks when more than 1.5 s apart. */
    private fun syncToRoom(s: be.alwyapps.watch.core.keep.TogetherState) {
        val p = player ?: return
        val expected =
            be.alwyapps.watch.core
                .togetherPosition(s, roomReceivedAt, android.os.SystemClock.elapsedRealtime())
                .toLong()
        applyingRoom = true
        try {
            if (p.playWhenReady != s.playing) p.playWhenReady = s.playing
            if (abs(p.currentPosition - expected) > 1500) p.seekTo(expected)
        } finally {
            applyingRoom = false
        }
    }

    /** The room moved to another title: find it on our servers and play it. */
    private fun followTitle(s: be.alwyapps.watch.core.keep.TogetherState) {
        val m = s.media
        lifecycleScope.launch {
            val accounts = runCatching { core.accounts() }.getOrDefault(emptyList()).map { it.id }
            val found =
                runCatching {
                    core.togetherFind(
                        be.alwyapps.watch.ui.screens
                            .preferredFirst(core, accounts),
                        m,
                    )
                }.getOrNull()
            if (found == null) {
                val what = m.seriesTitle?.let { "$it S${m.season} · E${m.episode}" } ?: m.title
                ui.togetherNote = "$what isn't on your servers"
                applyingRoom = true
                player?.pause()
                applyingRoom = false
                return@launch
            }
            report(PlaybackEvent.STOP)
            account = found.accountId
            announceTitle = false
            val start =
                be.alwyapps.watch.core
                    .togetherPosition(s, roomReceivedAt, android.os.SystemClock.elapsedRealtime())
                    .toLong()
            load(found.itemId, start)
        }
    }

    /** Tells the room what we just did (play, pause, seek; with [newTitle], the title too). */
    private fun sendRoom(newTitle: Boolean = false) {
        val code = room ?: return
        val me = member ?: return
        val p = player ?: return
        val playing = p.playWhenReady
        val position = p.currentPosition.coerceAtLeast(0).toULong()
        val (acct, item) = if (newTitle) account to itemId else null to null
        roomSending++
        lifecycleScope.launch {
            try {
                val s = core.togetherUpdate(code, me, playing, position, acct, item)
                acceptRoom(s)
                roomMedia = s.media
            } catch (e: CoreException) {
                android.util.Log.w(TAG, "together update: ${e.javaClass.simpleName}")
            } finally {
                roomSending--
            }
        }
    }

    private fun leaveTogether(
        note: String,
        tellServer: Boolean = true,
    ) {
        val code = room ?: return
        val me = member
        room = null
        member = null
        roomJob?.cancel()
        roomState = null
        roomMedia = null
        ui.togetherCode = null
        ui.togetherPeople = emptyList()
        ui.togetherNote = note
        if (tellServer && me != null) services.scope.launch { runCatching { core.togetherLeave(code, me) } }
    }

    // Another title started while this one plays (e.g. from the app behind a PiP window).
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        val acct = intent.getStringExtra(EXTRA_ACCOUNT) ?: return
        val id = intent.getStringExtra(EXTRA_ITEM) ?: return
        if (acct == account && id == itemId) return
        report(PlaybackEvent.STOP)
        account = acct
        announceTitle = room != null
        lifecycleScope.launch { load(id, intent.getLongExtra(EXTRA_START, 0L)) }
    }

    /** Show (or movie) name, then "S1 · E2 · Episode" or the year. Works offline. */
    private fun showTitle(id: String) {
        val cached = core.cachedItem(account, id)?.item
        if (cached != null) {
            showItem(cached)
            return
        }
        // Not cached one by one (started from an episode list): the download's title, then the server's.
        runCatching { core.downloads() }.getOrNull()?.firstOrNull { it.accountId == account && it.itemId == id }?.title?.let {
            ui.title = it.substringBefore(" · ")
            ui.subtitle = it.substringAfter(" · ", "")
        }
        lifecycleScope.launch {
            val fresh = runCatching { core.refreshItem(account, id, false).item }.getOrNull()
            if (fresh != null && itemId == id) showItem(fresh)
        }
    }

    private fun showItem(item: be.alwyapps.watch.core.media.Item) {
        if (item.kind == ItemKind.EPISODE) {
            ui.title = item.seriesTitle ?: item.title
            ui.subtitle =
                listOfNotNull(
                    if (item.seasonNumber != null && item.episodeNumber != null) "S${item.seasonNumber} · E${item.episodeNumber}" else null,
                    item.title.takeIf { item.seriesTitle != null },
                ).joinToString(" · ")
        } else {
            ui.title = item.title
            ui.subtitle = item.year?.toString() ?: ""
        }
    }

    private val pipSupported by lazy {
        !services.isTv && packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_PICTURE_IN_PICTURE)
    }

    private fun pipParams(): android.app.PictureInPictureParams {
        val size = player?.videoSize
        // Android accepts 1:2.39 to 2.39:1.
        val ratio =
            if (size != null && size.width > 0 && size.height > 0) {
                val w = size.width * size.pixelWidthHeightRatio
                android.util.Rational(
                    w.toInt().coerceIn((size.height / 2.39f).toInt() + 1, (size.height * 2.39f).toInt()),
                    size.height,
                )
            } else {
                android.util.Rational(16, 9)
            }
        return android.app.PictureInPictureParams
            .Builder()
            .setAspectRatio(ratio)
            .build()
    }

    private fun enterPip() {
        if (!pipSupported) return
        ui.visible = false
        runCatching { enterPictureInPictureMode(pipParams()) }
    }

    // Home button while a video plays: keep watching in a small window.
    override fun onUserLeaveHint() {
        super.onUserLeaveHint()
        if (player?.isPlaying == true) enterPip()
    }

    override fun onPictureInPictureModeChanged(
        isInPictureInPictureMode: Boolean,
        newConfig: android.content.res.Configuration,
    ) {
        super.onPictureInPictureModeChanged(isInPictureInPictureMode, newConfig)
        ui.pip = isInPictureInPictureMode
        if (!isInPictureInPictureMode) ui.poke()
    }

    override fun onStop() {
        super.onStop()
        val stop = report(PlaybackEvent.STOP)
        // Once the server knows where we stopped, the screens behind show "Resume" at the right place.
        services.scope.launch {
            stop?.join()
            kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Main) {
                be.alwyapps.watch.ui.ItemsChanged
                    .bump()
            }
        }
        // Watched episode: its download may be due for deletion, the next one for download.
        if (!services.isTv) {
            services.scope.launch {
                kotlinx.coroutines.delay(1500)
                services.syncDownloads(core)
            }
        }
        leaveTogether("")
        progressJob?.cancel()
        segmentJob?.cancel()
        player?.release()
        player = null
        window.attributes = window.attributes.apply { preferredDisplayModeId = originalModeId }
        finish()
    }

    companion object {
        private const val EXTRA_ACCOUNT = "account"
        private const val EXTRA_ITEM = "item"
        private const val EXTRA_START = "start"
        private const val EXTRA_ROOM = "together_room"
        private const val EXTRA_MEMBER = "together_member"
        private const val TAG = "AlwyPlayer"
        private const val BACK_GRACE_MS = 2000L
        private const val BACK_ECHO_MS = 300L
        private const val COUNTDOWN_S = 10
        private val AUTO_SKIP = setOf(SegmentKind.INTRO, SegmentKind.RECAP)

        fun intent(
            ctx: Context,
            account: String,
            itemId: String,
            startMs: Long,
            room: String? = null,
            member: String? = null,
        ) = Intent(ctx, PlayerActivity::class.java)
            .putExtra(EXTRA_ACCOUNT, account)
            .putExtra(EXTRA_ITEM, itemId)
            .putExtra(EXTRA_START, startMs)
            .putExtra(EXTRA_ROOM, room)
            .putExtra(EXTRA_MEMBER, member)
    }
}
