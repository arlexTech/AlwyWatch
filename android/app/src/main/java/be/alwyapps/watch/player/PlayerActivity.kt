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
import be.alwyapps.watch.core.media.PlayMethod
import be.alwyapps.watch.core.media.PlaybackEvent
import be.alwyapps.watch.core.media.PlaybackInfo
import be.alwyapps.watch.core.media.ProgressReport
import be.alwyapps.watch.core.media.Segment
import be.alwyapps.watch.core.media.SegmentKind
import be.alwyapps.watch.core.media.StreamKind
import be.alwyapps.watch.core.media.SubtitleSelection
import be.alwyapps.watch.core.media.TrackSelection
import be.alwyapps.watch.services
import be.alwyapps.watch.together.TogetherInbox
import be.alwyapps.watch.together.TogetherSession
import be.alwyapps.watch.together.friendly
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
    private var countdownS = 10

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

    /** The tracks the server's conversion was asked with (null: direct play, or the server's own choice). */
    private var conversion: TrackSelection? = null
    private val ui = PlayerUi()

    // ----- watch together: the room this player follows (null: alone) -----
    // The seat itself lives in TogetherSession (shared with the Room screen, which opened us).
    private var together = false
    private val room: String? get() = if (together) TogetherSession.seat?.code else null
    private val member: String? get() = if (together) TogetherSession.seat?.memberId else null
    private val roomState: be.alwyapps.watch.core.keep.TogetherState? get() = if (together) TogetherSession.state else null
    private var roomMedia: be.alwyapps.watch.core.keep.TogetherMedia? = null

    /** The room's start (`round`) this player plays, and the last one we said we loaded. */
    private var roomRound = 0uL
    private var readySent = 0uL

    /** Since when the player could start, while the room waits for everyone to load. */
    private var readySince = 0L

    /** The start (server time) a timer presses play for, and that timer. */
    private var scheduledStart = 0L
    private var startJob: Job? = null

    /** Where the player was when "Watch together" opened the room dialog. */
    private var dialogPosition = 0L
    private var roomAttached = false

    /** The last room state handled here (to say who did what). */
    private var lastSeen: be.alwyapps.watch.core.keep.TogetherState? = null
    private var roomJob: Job? = null

    /** Applying the room's state (or starting a title): the player's events are not ours to send. */
    private var applyingRoom = false

    /** Our own updates on their way: the room answers in between are older than them. */
    private var roomSending = 0

    /** This title was chosen here (next episode…): tell the room once it starts. */
    private var announceTitle = false

    /** The movie or show whose audio/subtitle choice applies, and whether it was applied yet. */
    private var trackKey: String = ""
    private var tracksApplied = false

    private var subtitleLift: Job? = null

    /** Last remote/touch interaction (uptime), for "Are you still watching?". */
    private var lastInteraction = android.os.SystemClock.uptimeMillis()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        account = intent.getStringExtra(EXTRA_ACCOUNT) ?: return finish()
        itemId = intent.getStringExtra(EXTRA_ITEM) ?: return finish()
        val roomCode = intent.getStringExtra(EXTRA_ROOM)
        val roomMember = intent.getStringExtra(EXTRA_MEMBER)
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
                            onLeaveTogether = { leaveTogether("You left watch together") },
                            onStartTogether = { watchTogether() },
                            onTrackPicked = { type, format -> rememberTrack(type, format) },
                            onServerTrackPicked = { kind, index -> pickServerTrack(kind, index) },
                            onStillWatching = {
                                ui.stillWatching = false
                                lastInteraction = android.os.SystemClock.uptimeMillis()
                                player?.play()
                            },
                        )
                        if (ui.roomDialog && ::core.isInitialized) {
                            androidx.compose.runtime.CompositionLocalProvider(be.alwyapps.watch.ui.LocalCore provides core) {
                                be.alwyapps.watch.ui.screens.PlayerRoomDialog(
                                    dialogPosition,
                                    onStart = { startFromDialog() },
                                    onCancel = { cancelRoomDialog() },
                                )
                            }
                        }
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
            // New invitations while watching: a short note (the full prompt waits in the app).
            launch { TogetherInbox.notes.collect { ui.togetherNote = it } }
            if (roomCode != null && roomMember != null) {
                TogetherSession.adopt(core, roomCode, roomMember)
                together = true
            }
            ui.canWatchTogether = !together && runCatching { core.keepActive() }.getOrDefault(false)
            load(itemId, start)
            if (together) followRoom()
        }
    }

    private suspend fun load(
        id: String,
        startMs: Long,
    ) {
        itemId = id
        // Audio/subtitles follow the movie's or the show's choice.
        val item =
            core.cachedItem(account, id)?.item
                ?: kotlinx.coroutines.withTimeoutOrNull(1500) { runCatching { core.refreshItem(account, id, false).item }.getOrNull() }
        trackKey = item?.let { TrackPrefs.titleKey(it) } ?: id
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
        // The version chosen on the title page (4K, 1080p…), else the server's first.
        val chosen = TrackPrefs.version(core, account, id)
        val src = pi.sources.firstOrNull { it.id == chosen } ?: pi.sources.firstOrNull()
        if (src == null) {
            Toast.makeText(this, "Nothing to play", Toast.LENGTH_LONG).show()
            finish()
            return
        }
        info = pi
        source = src
        if (DeviceProfiles.canDirectPlay(src, profile) || src.transcode == null) {
            startPlayer(src.direct, src, transcoding = false, startMs)
        } else {
            startConversion(src, startMs, null)
        }
    }

    /**
     * Plays the server's conversion of [src] carrying [selection], else the tracks the title's
     * choice or the settings pick (the same rules as direct play, over the file's streams): a
     * converted stream has a single audio track and burned-in subtitles, so the server must
     * know the choice before it starts. If that request fails, the plain conversion plays.
     */
    private suspend fun startConversion(
        src: MediaSource,
        startMs: Long,
        selection: TrackSelection?,
        rewind: Boolean = true,
    ) {
        val id = itemId
        val sel =
            selection ?: be.alwyapps.watch.core.conversionTracks(
                TrackPrefs.get(core, account, trackKey),
                trackRules(PlayerSettings(core, services.isTv), null),
                src,
            )
        val pi =
            runCatching { core.playbackInfoWith(account, id, DeviceProfiles.get(this), sel) }
                .onFailure { android.util.Log.w(TAG, "conversion with tracks: ${it.javaClass.simpleName}") }
                .getOrNull()
        if (itemId != id) return
        val converted = pi?.sources?.firstOrNull { it.id == src.id }
        val req = converted?.transcode
        if (pi == null || converted == null || req == null) {
            conversion = null
            ui.serverTracks = null
            startPlayer(src.transcode ?: return, src, transcoding = true, startMs, rewind = rewind)
            return
        }
        info = pi
        source = converted
        conversion = sel
        ui.serverTracks = ServerTracks(converted.streams, sel)
        startPlayer(req, converted, transcoding = true, startMs, rewind = rewind)
    }

    /** A track picked in the panel while converting: remembered, then the conversion restarts with it, here. */
    private fun pickServerTrack(
        kind: StreamKind,
        index: Long?,
    ) {
        val src = source ?: return
        val sel = conversion ?: return
        val pos = index?.let { i -> src.streams.indexOfFirst { it.kind == kind && it.index == i }.takeIf { it >= 0 } }
        if (index != null && pos == null) return
        val trackKind = if (kind == StreamKind.AUDIO) be.alwyapps.watch.core.TrackKind.AUDIO else be.alwyapps.watch.core.TrackKind.SUBTITLE
        val pref =
            be.alwyapps.watch.core
                .pickTrack(
                    TrackPrefs.get(core, account, trackKey),
                    trackKind,
                    be.alwyapps.watch.core
                        .sourceTracks(src),
                    pos?.toUInt(),
                )
        TrackPrefs.set(core, account, trackKey, pref)
        val next =
            when (kind) {
                StreamKind.AUDIO -> sel.copy(audioIndex = index ?: return)
                else -> sel.copy(subtitle = index?.let { SubtitleSelection.Stream(it) } ?: SubtitleSelection.Off)
            }
        if (next == sel) return
        val position = player?.currentPosition?.coerceAtLeast(0) ?: 0L
        lifecycleScope.launch {
            // Ends this conversion on the server (Plex stops converting a session reported stopped).
            report(PlaybackEvent.STOP)?.join()
            startConversion(src, position, next, rewind = false)
        }
    }

    private fun startPlayer(
        req: HttpRequest,
        src: MediaSource,
        transcoding: Boolean,
        startMs: Long,
        rewind: Boolean = true,
    ) {
        player?.release()
        this.transcoding = transcoding
        if (!transcoding) {
            conversion = null
            ui.serverTracks = null
        }
        // Tracks picked by the server (burned in): nothing for the player to choose or add.
        val serverPicks = transcoding && conversion != null
        tracksApplied = serverPicks
        val settings = PlayerSettings(core, services.isTv)
        styleSubtitles(settings)
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
                .setTrackTypeDisabled(C.TRACK_TYPE_TEXT, serverPicks)
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
                .setSeekBackIncrementMs(settings.seekBackS * 1000L)
                .setSeekForwardIncrementMs(settings.seekForwardS * 1000L)
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
                        .filter { !serverPicks && it.kind == StreamKind.SUBTITLE && it.external != null }
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
        // Resuming: a few seconds again, to pick up the thread (not in a room: it sets the time).
        val from = if (rewind && room == null && startMs > 0) (startMs - settings.resumeRewindS * 1000L).coerceAtLeast(0) else startMs
        p.setMediaItem(item, from)
        p.prepare()
        // In a room that waits for everyone to load (or starts in a moment): paused until then.
        p.playWhenReady = roomState?.let {
            it.playing &&
                it.loading.isEmpty() &&
                be.alwyapps.watch.core
                    .togetherStartsIn(it, core.togetherNowMs()) == null
        } ?: true
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
                    checkStillWatching(pl)
                    maybePrefetchNext(pl)
                }
            }
        segmentJob?.cancel()
        segmentJob =
            lifecycleScope.launch {
                while (isActive) {
                    player?.let { watchSegments(it) }
                    roomState?.let { syncToRoom(it) }
                    delay(500)
                }
            }
    }

    private val listener =
        object : Player.Listener {
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                ui.playing = isPlaying
                refreshPip()
                // Paused by the user: show the controls. Not while buffering after a seek
                // (←/→ with hidden controls must keep them hidden).
                if (!isPlaying && player?.playWhenReady == false) ui.poke()
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
                    lifecycleScope.launch { startConversion(src, position, null) }
                } else {
                    Toast.makeText(this@PlayerActivity, "Playback failed: ${error.errorCodeName}", Toast.LENGTH_LONG).show()
                }
            }

            override fun onTracksChanged(tracks: Tracks) {
                val p = player
                if (!tracksApplied && p != null && !tracks.groups.isEmpty()) {
                    tracksApplied = true
                    runCatching { applyTrackPrefs(p, TrackPrefs.get(core, account, trackKey), PlayerSettings(core, services.isTv)) }
                }
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
            countdownS = it.countdownS
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
            val left = countdownS - ((android.os.SystemClock.uptimeMillis() - from) / 1000).toInt()
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
                // Converting with chosen tracks: the server's streams it carries.
                audioIndex = conversion?.audioIndex,
                subtitleIndex = (conversion?.subtitle as? SubtitleSelection.Stream)?.index,
                playMethod = if (transcoding) PlayMethod.TRANSCODE else PlayMethod.DIRECT,
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

    override fun dispatchTouchEvent(ev: android.view.MotionEvent): Boolean {
        lastInteraction = android.os.SystemClock.uptimeMillis()
        return super.dispatchTouchEvent(ev)
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val down = event.action == KeyEvent.ACTION_DOWN
        lastInteraction = android.os.SystemClock.uptimeMillis()
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
                    // Seek without opening the controls, so presses add up (−10 s, +30 s each);
                    // a slim bar shows where it lands.
                    if (down && p != null) {
                        val step = if (event.keyCode == KeyEvent.KEYCODE_DPAD_LEFT) -p.seekBackIncrement else p.seekForwardIncrement
                        val base = ui.seekTarget ?: p.currentPosition
                        val dur = p.duration.takeIf { it > 0 } ?: Long.MAX_VALUE
                        val target = (base + step).coerceIn(0, dur)
                        ui.seekTarget = target
                        p.seekTo(target)
                        ui.seekFlash++
                    }
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

    /** Subtitle size, colour, background and position from the settings (not ASS styling). */
    private fun styleSubtitles(settings: PlayerSettings) {
        val sv = view.subtitleView ?: return
        val fg = if (settings.subtitleColor == "yellow") android.graphics.Color.rgb(255, 230, 80) else android.graphics.Color.WHITE
        val bg = if (settings.subtitleBackground) android.graphics.Color.argb(170, 0, 0, 0) else android.graphics.Color.TRANSPARENT
        sv.setStyle(
            androidx.media3.ui.CaptionStyleCompat(
                fg,
                bg,
                android.graphics.Color.TRANSPARENT,
                // No box: a dark outline keeps them readable on bright scenes (like Plex/Netflix).
                if (settings.subtitleBackground) {
                    androidx.media3.ui.CaptionStyleCompat.EDGE_TYPE_NONE
                } else {
                    androidx.media3.ui.CaptionStyleCompat.EDGE_TYPE_OUTLINE
                },
                android.graphics.Color.argb(220, 0, 0, 0),
                android.graphics.Typeface.create("sans-serif-medium", android.graphics.Typeface.NORMAL),
            ),
        )
        val scale =
            when (settings.subtitleSize) {
                "small" -> 0.8f
                "large" -> 1.25f
                "huge" -> 1.5f
                else -> 1f
            }
        sv.setFractionalTextSize(androidx.media3.ui.SubtitleView.DEFAULT_TEXT_SIZE_FRACTION * scale)
        // Measured from the bottom of the picture: low by default (Media3's 8% sat too high),
        // above the seek bar while the controls are shown.
        val base = if (settings.subtitleRaised) 0.12f else 0.04f
        sv.setBottomPaddingFraction(if (ui.visible) 0.24f else base)
        subtitleLift?.cancel()
        subtitleLift =
            lifecycleScope.launch {
                androidx.compose.runtime
                    .snapshotFlow { ui.visible to (ui.seekTarget != null) }
                    .collect { (shown, seeking) ->
                        sv.setBottomPaddingFraction(
                            when {
                                shown -> 0.24f
                                seeking -> 0.12f
                                else -> base
                            },
                        )
                        // Redraw the cue on screen now, not at the next one.
                        player?.let { sv.setCues(it.currentCues.cues) }
                    }
            }
    }

    /** A track picked in the player becomes the movie's or show's choice (next episodes too). */
    private fun rememberTrack(
        type: Int,
        format: androidx.media3.common.Format?,
    ) {
        // The core remembers its title and position among same-language tracks, so the next
        // episodes of a pack with the same layout get the same track.
        val tracks = PlayerTracks(player?.currentTracks ?: return)
        val kind = if (type == C.TRACK_TYPE_AUDIO) be.alwyapps.watch.core.TrackKind.AUDIO else be.alwyapps.watch.core.TrackKind.SUBTITLE
        val picked = format?.let { tracks.idOf(it) }
        if (format != null && picked == null) return
        val pref =
            be.alwyapps.watch.core
                .pickTrack(TrackPrefs.get(core, account, trackKey), kind, tracks.infos, picked)
        TrackPrefs.set(core, account, trackKey, pref)
    }

    /** Pauses after hours without touching the remote, like Plex (not in a room: others watch). */
    private fun checkStillWatching(p: Player) {
        val hours = PlayerSettings(core, services.isTv).stillWatchingH
        if (hours <= 0 || room != null || !p.isPlaying || ui.stillWatching) return
        if (android.os.SystemClock.uptimeMillis() - lastInteraction < hours * 3_600_000L) return
        p.pause()
        ui.stillWatching = true
    }

    // ----- watch together ------------------------------------------------------------------

    /**
     * "Watch together" while watching alone: pauses and opens a private room with this title in
     * a dialog over the video (the host invites people there, then starts for everyone from
     * here, without leaving the player).
     */
    private fun watchTogether() {
        if (together || ui.roomDialog) return
        val p = player
        p?.pause()
        dialogPosition = p?.currentPosition?.coerceAtLeast(0) ?: 0L
        ui.roomDialog = true
        lifecycleScope.launch {
            try {
                val joined = core.togetherCreate(account, itemId)
                if (TogetherSession.seat != null && TogetherSession.seat?.code != joined.state.code) TogetherSession.leave()
                TogetherSession.begin(core, joined, startAt = dialogPosition)
            } catch (e: CoreException) {
                ui.roomDialog = false
                ui.togetherNote = friendly(e)
            }
        }
    }

    /** The dialog's "Start": everyone loads this title here, then it plays for all at once. */
    private fun startFromDialog() {
        val seat = TogetherSession.seat ?: return
        val pos = player?.currentPosition?.coerceAtLeast(0) ?: dialogPosition
        lifecycleScope.launch {
            try {
                val s = core.togetherStart(seat.code, seat.memberId, pos.toULong())
                TogetherSession.accept(s)
                TogetherSession.handledRound = maxOf(TogetherSession.handledRound, s.round)
                together = true
                roomRound = s.round
                ui.roomDialog = false
                ui.canWatchTogether = false
                followRoom()
                syncToRoom(s)
            } catch (e: CoreException) {
                ui.togetherNote = friendly(e)
            }
        }
    }

    /** The dialog's "Cancel": the room closes, playback goes on alone. */
    private fun cancelRoomDialog() {
        ui.roomDialog = false
        TogetherSession.leave()
        player?.play()
    }

    private fun showRoom(s: be.alwyapps.watch.core.keep.TogetherState) {
        ui.togetherCode = s.code
        ui.togetherMembers = s.members
    }

    /** Follows the room (its long poll runs in TogetherSession: we keep it alive while shown). */
    private fun followRoom() {
        val code = room ?: return
        roomJob?.cancel()
        if (!roomAttached) {
            roomAttached = true
            TogetherSession.attach(core)
        }
        roomState?.let {
            showRoom(it)
            lastSeen = it
            roomMedia = it.media
            roomRound = it.round
            TogetherSession.handledRound = maxOf(TogetherSession.handledRound, it.round)
        }
        ui.togetherCode = code
        roomJob =
            lifecycleScope.launch {
                launch { TogetherSession.updates.collect { if (it.code == code) onRoomState(it) } }
                // The room closed, or this device moved to another one.
                launch {
                    androidx.compose.runtime
                        .snapshotFlow { TogetherSession.seat?.code }
                        .collect { now ->
                            if (now != code) {
                                val why = TogetherSession.ended
                                TogetherSession.ended = null
                                leaveTogether(why ?: "You're no longer in this room", tellServer = false)
                            }
                        }
                }
            }
    }

    private fun onRoomState(s: be.alwyapps.watch.core.keep.TogetherState) {
        val before = lastSeen
        lastSeen = s
        showRoom(s)
        // The host went back to the waiting room (to pick something else): so do we.
        if (!s.started) {
            if (!TogetherSession.isHost()) {
                val host = s.members.firstOrNull { it.id == s.host }?.name ?: "The host"
                TogetherSession.problem = null
                exitRoomPlayer()
                android.widget.Toast
                    .makeText(this, "$host went back to the waiting room", android.widget.Toast.LENGTH_SHORT)
                    .show()
                finish()
            }
            return
        }
        val newRound = roomRound != 0uL && s.round != roomRound
        roomRound = s.round
        TogetherSession.handledRound = maxOf(TogetherSession.handledRound, s.round)
        if (s.by == member || roomSending > 0) {
            roomMedia = s.media
            if (s.loading.isNotEmpty()) syncToRoom(s)
            return
        }
        val known = roomMedia
        roomMedia = s.media
        if (newRound || (known != null && known != s.media)) {
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
            } else if (s.members.size > before.members.size) {
                val joined = s.members.filter { m -> before.members.none { it.id == m.id } }.joinToString(", ") { it.name }
                ui.togetherNote = "$joined joined"
            } else if (s.members.size < before.members.size) {
                val left = before.members.filter { m -> s.members.none { it.id == m.id } }.joinToString(", ") { it.name }
                ui.togetherNote = "$left left"
            }
        }
        // Someone else's change wins over our own pending hold.
        ownHoldUntil = 0L
        syncToRoom(s)
    }

    /** Speed used to catch up with the room (1.0: normal), see [syncToRoom]. */
    private var roomRate = 1.0

    /** False once a speed change didn't close the gap (passthrough audio): seeks only. */
    private var rateWorks = true
    private var rateCheckAt = 0L
    private var rateCheckDrift = 0L

    /** No following before this: our own change, not echoed by the room yet. */
    private var ownHoldUntil = 0L

    /** No corrections before this: a seek landing. */
    private var seekHoldUntil = 0L

    /**
     * Plays or pauses like the room and stays in step with it, every half second: small drifts
     * by playing slightly faster or slower, a seek past 2 s (the core's `togetherCorrection`).
     */
    private fun syncToRoom(s: be.alwyapps.watch.core.keep.TogetherState) {
        val p = player ?: return
        val now = android.os.SystemClock.elapsedRealtime()
        val serverNow = core.togetherNowMs()
        if (roomSending > 0) return
        if (s.loading.isNotEmpty() ||
            be.alwyapps.watch.core
                .togetherStartsIn(s, serverNow) != null
        ) {
            waitForStart(p, s, serverNow)
            return
        }
        ui.togetherWaiting = null
        val expected =
            be.alwyapps.watch.core
                .togetherPositionAt(s, serverNow)
                .toLong()
        if (now < ownHoldUntil) return
        applyingRoom = true
        try {
            if (p.playWhenReady != s.playing) p.playWhenReady = s.playing
            if (now < seekHoldUntil || p.playbackState != Player.STATE_READY) return
            val drift = p.currentPosition - expected
            val c =
                be.alwyapps.watch.core.togetherCorrection(
                    expected.coerceAtLeast(0).toULong(),
                    p.currentPosition.coerceAtLeast(0).toULong(),
                    s.playing,
                    roomRate,
                    rateWorks,
                )
            c.seekToMs?.let {
                p.seekTo(it.toLong())
                seekHoldUntil = now + 2_000
            }
            if (c.rate != roomRate) {
                if (roomRate == 1.0) {
                    rateCheckAt = now + 6_000
                    rateCheckDrift = abs(drift)
                }
                roomRate = c.rate
                p.setPlaybackSpeed(c.rate.toFloat())
            } else if (roomRate != 1.0 && now >= rateCheckAt) {
                // Passthrough audio ignores the speed: the gap doesn't close. Seek instead.
                if (abs(drift) > rateCheckDrift * 8 / 10) {
                    rateWorks = false
                    roomRate = 1.0
                    p.setPlaybackSpeed(1f)
                }
                rateCheckAt = now + 6_000
                rateCheckDrift = abs(drift)
            }
        } finally {
            applyingRoom = false
        }
    }

    /**
     * A start: every seat loads the title paused at the room's position and says when it has
     * enough ([be.alwyapps.watch.core.togetherReadyBufferMs]); once all have, the room plays from
     * a moment ahead and a timer presses play right then, here as everywhere else.
     */
    private fun waitForStart(
        p: Player,
        s: be.alwyapps.watch.core.keep.TogetherState,
        serverNow: Long,
    ) {
        val now = android.os.SystemClock.elapsedRealtime()
        val me = member ?: return
        val at = s.positionMs.toLong()
        applyingRoom = true
        try {
            if (p.playWhenReady) p.playWhenReady = false
            if (roomRate != 1.0) {
                roomRate = 1.0
                p.setPlaybackSpeed(1f)
            }
            if (now >= seekHoldUntil && p.playbackState == Player.STATE_READY && abs(p.currentPosition - at) > 150) {
                p.seekTo(at)
                seekHoldUntil = now + 500
            }
        } finally {
            applyingRoom = false
        }
        val startsIn =
            be.alwyapps.watch.core
                .togetherStartsIn(s, serverNow)
        if (startsIn != null) {
            ui.togetherWaiting = "Starting…"
            if (scheduledStart != s.atMs) {
                scheduledStart = s.atMs
                startJob?.cancel()
                startJob =
                    lifecycleScope.launch {
                        delay(startsIn)
                        val st = roomState ?: return@launch
                        if (st.atMs != s.atMs || !st.playing) return@launch
                        applyingRoom = true
                        player?.play()
                        applyingRoom = false
                        ui.togetherWaiting = null
                    }
            }
            return
        }
        val mine =
            be.alwyapps.watch.core
                .togetherIsLoading(s, me)
        if (mine && readySent != s.round) {
            val ready = p.playbackState == Player.STATE_READY && abs(p.currentPosition - at) <= 500
            if (!ready) {
                readySince = 0L
            } else if (readySince == 0L) {
                readySince = now
            }
            val ahead = p.bufferedPosition - p.currentPosition
            val dur = p.duration
            val enough =
                ready &&
                    (
                        ahead >=
                            be.alwyapps.watch.core
                                .togetherReadyBufferMs()
                                .toLong() ||
                            (dur > 0 && p.bufferedPosition >= dur - 1_000) ||
                            now - readySince > 8_000
                    )
            if (enough) {
                val round = s.round
                readySent = round
                val code = room ?: return
                lifecycleScope.launch {
                    try {
                        TogetherSession.accept(core.togetherReady(code, me, round))
                    } catch (e: CoreException) {
                        android.util.Log.w(TAG, "together ready: ${e.javaClass.simpleName}")
                        if (readySent == round) readySent = 0uL
                    }
                }
            }
        }
        val others =
            s.loading
                .filter { it != me }
                .mapNotNull { id -> s.members.firstOrNull { it.id == id }?.name }
                .distinct()
        ui.togetherWaiting =
            when {
                mine && readySent != s.round -> "Loading for everyone…"
                others.isEmpty() -> "Starting…"
                else -> "Waiting for ${others.joinToString(", ")}…"
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
                    .togetherPositionAt(s, core.togetherNowMs())
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
        ownHoldUntil = android.os.SystemClock.elapsedRealtime() + 2_000
        roomSending++
        lifecycleScope.launch {
            try {
                val s = core.togetherUpdate(code, me, playing, position, acct, item)
                TogetherSession.accept(s)
                lastSeen = s
                showRoom(s)
                roomMedia = s.media
            } catch (e: CoreException) {
                android.util.Log.w(TAG, "together update: ${e.javaClass.simpleName}")
                // The host doesn't allow it (HTTP 423): say so and go back to what the room plays.
                val s = TogetherSession.state
                if (e is CoreException.Server && e.message?.contains("HTTP 423") == true && s != null) {
                    val what =
                        when {
                            newTitle -> "change what's playing"
                            s.playing != playing -> if (playing) "play" else "pause"
                            else -> "move forward or back"
                        }
                    ui.togetherNote = "Only the host can $what in this room"
                    ownHoldUntil = 0L
                    if (newTitle) followTitle(s) else syncToRoom(s)
                }
            } finally {
                roomSending--
            }
        }
    }

    private fun leaveTogether(
        note: String,
        tellServer: Boolean = true,
    ) {
        if (!together) return
        together = false
        roomJob?.cancel()
        roomJob = null
        startJob?.cancel()
        ui.togetherWaiting = null
        if (roomAttached) {
            roomAttached = false
            TogetherSession.detach()
        }
        roomMedia = null
        lastSeen = null
        ui.togetherCode = null
        ui.togetherMembers = emptyList()
        ui.togetherNote = note
        if (tellServer) TogetherSession.leave()
    }

    /**
     * The player closes while in a room: we stay in it (the app shows the Room screen). The
     * host closing it takes everyone back to the waiting room, to pick something else.
     */
    private fun exitRoomPlayer() {
        if (!together) return
        val seat = TogetherSession.seat
        val s = TogetherSession.state
        val hosting = TogetherSession.isHost()
        val pos = player?.currentPosition?.coerceAtLeast(0) ?: 0L
        together = false
        roomJob?.cancel()
        roomJob = null
        startJob?.cancel()
        ui.togetherWaiting = null
        if (roomAttached) {
            roomAttached = false
            TogetherSession.detach()
        }
        if (seat == null || s == null) return
        TogetherSession.handledRound = maxOf(TogetherSession.handledRound, s.round)
        if (hosting && s.started) {
            TogetherSession.startAt = pos
            val c = core
            services.scope.launch {
                runCatching { c.togetherLobby(seat.code, seat.memberId, pos.toULong()) }.onSuccess {
                    kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Main) { TogetherSession.accept(it) }
                }
            }
        }
        TogetherInbox.openRoom = seat.code
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
        lifecycleScope.launch {
            load(id, intent.getLongExtra(EXTRA_START, 0L))
        }
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
            .setActions(pipActions())
            .apply {
                // Android 12+: the swipe home glides into the small window while playing.
                if (android.os.Build.VERSION.SDK_INT >= 31) setAutoEnterEnabled(player?.isPlaying == true)
            }.build()
    }

    /** The small window's buttons: back 10 s, play/pause, forward 30 s. */
    private fun pipActions(): List<android.app.RemoteAction> {
        fun action(
            code: Int,
            icon: Int,
            label: String,
        ) = android.app.RemoteAction(
            android.graphics.drawable.Icon
                .createWithResource(this, icon),
            label,
            label,
            android.app.PendingIntent.getBroadcast(
                this,
                code,
                android.content
                    .Intent(PIP_ACTION)
                    .setPackage(packageName)
                    .putExtra(PIP_EXTRA, code),
                android.app.PendingIntent.FLAG_UPDATE_CURRENT or android.app.PendingIntent.FLAG_IMMUTABLE,
            ),
        )
        val playing = player?.isPlaying == true
        return listOf(
            action(PIP_BACK, android.R.drawable.ic_media_rew, "Back 10 seconds"),
            if (playing) {
                action(PIP_PAUSE, android.R.drawable.ic_media_pause, "Pause")
            } else {
                action(PIP_PLAY, android.R.drawable.ic_media_play, "Play")
            },
            action(PIP_FORWARD, android.R.drawable.ic_media_ff, "Forward 30 seconds"),
        )
    }

    private val pipReceiver =
        object : android.content.BroadcastReceiver() {
            override fun onReceive(
                context: android.content.Context?,
                intent: android.content.Intent?,
            ) {
                val p = player ?: return
                when (intent?.getIntExtra(PIP_EXTRA, 0)) {
                    PIP_PLAY -> p.play()
                    PIP_PAUSE -> p.pause()
                    PIP_BACK -> p.seekBack()
                    PIP_FORWARD -> p.seekForward()
                }
            }
        }
    private var pipReceiverOn = false

    /** Keeps the small window's play/pause button (and the auto-enter rule) in step. */
    private fun refreshPip() {
        if (pipSupported) runCatching { setPictureInPictureParams(pipParams()) }
    }

    private fun enterPip() {
        if (!pipSupported) return
        if (android.os.Build.VERSION.SDK_INT >= 31 && isInPictureInPictureMode) return
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

    override fun onDestroy() {
        if (pipReceiverOn) runCatching { unregisterReceiver(pipReceiver) }
        super.onDestroy()
    }

    override fun onStart() {
        super.onStart()
        if (pipSupported && !pipReceiverOn) {
            androidx.core.content.ContextCompat.registerReceiver(
                this,
                pipReceiver,
                android.content.IntentFilter(PIP_ACTION),
                androidx.core.content.ContextCompat.RECEIVER_NOT_EXPORTED,
            )
            pipReceiverOn = true
            refreshPip()
        }
        TogetherInbox.activityStarted(null, player = true)
    }

    override fun onStop() {
        super.onStop()
        TogetherInbox.activityStopped(player = true)
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
        exitRoomPlayer()
        // "Watch together" pressed, never started: that room closes.
        if (ui.roomDialog) {
            ui.roomDialog = false
            TogetherSession.leave()
        }
        progressJob?.cancel()
        segmentJob?.cancel()
        player?.release()
        player = null
        window.attributes = window.attributes.apply { preferredDisplayModeId = originalModeId }
        finish()
    }

    companion object {
        private const val PIP_ACTION = "be.alwyapps.watch.player.PIP"
        private const val PIP_EXTRA = "do"
        private const val PIP_PLAY = 1
        private const val PIP_PAUSE = 2
        private const val PIP_BACK = 3
        private const val PIP_FORWARD = 4
        private const val EXTRA_ACCOUNT = "account"
        private const val EXTRA_ITEM = "item"
        private const val EXTRA_START = "start"
        private const val EXTRA_ROOM = "together_room"
        private const val EXTRA_MEMBER = "together_member"
        private const val TAG = "AlwyPlayer"
        private const val BACK_GRACE_MS = 2000L
        private const val BACK_ECHO_MS = 300L
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
