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
    private var prefetchedNext = false
    private var originalModeId = 0
    private var transcoding = false
    private val ui = PlayerUi()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        account = intent.getStringExtra(EXTRA_ACCOUNT) ?: return finish()
        itemId = intent.getStringExtra(EXTRA_ITEM) ?: return finish()
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
                            onPip = if (pipSupported) ({ enterPip() }) else null,
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
            load(itemId, start)
        }
    }

    private suspend fun load(
        id: String,
        startMs: Long,
    ) {
        itemId = id
        showTitle(id)
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
            findNextEpisode()
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
        findNextEpisode()
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
        p.setMediaItem(item, startMs)
        p.prepare()
        p.playWhenReady = true
        view.player = p
        player = p
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
    }

    private val listener =
        object : Player.Listener {
            override fun onIsPlayingChanged(isPlaying: Boolean) {
                ui.playing = isPlaying
                if (!isPlaying) ui.poke()
                val p = player ?: return
                if (!isPlaying && p.playbackState == Player.STATE_READY) report(PlaybackEvent.PAUSE)
            }

            override fun onPlaybackStateChanged(state: Int) {
                ui.buffering = state == Player.STATE_BUFFERING || state == Player.STATE_IDLE
                if (state == Player.STATE_ENDED) {
                    report(PlaybackEvent.STOP)
                    val next = nextEpisodeId
                    if (next != null) lifecycleScope.launch { load(next, 0L) } else finish()
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

    private fun findNextEpisode() {
        nextEpisodeId = null
        prefetchedNext = false
        val item = core.cachedItem(account, itemId)?.item ?: return
        if (item.kind != ItemKind.EPISODE) return
        val series = item.seriesId ?: return
        val season = item.seasonId ?: return
        val eps = core.cachedEpisodes(account, series, season)?.items ?: return
        val idx = eps.indexOfFirst { it.id == itemId }
        nextEpisodeId = eps.getOrNull(idx + 1)?.id
        ui.hasNext = nextEpisodeId != null
    }

    private fun maybePrefetchNext(p: Player) {
        val next = nextEpisodeId ?: return
        if (prefetchedNext || p.duration <= 0) return
        if (p.currentPosition > p.duration * 0.8) {
            prefetchedNext = true
            core.prefetchPlaybackInfo(account, next, DeviceProfiles.get(this))
        }
    }

    private fun report(event: PlaybackEvent) {
        val p = player ?: return
        val src = source ?: return
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
        services.scope.launch { runCatching { core.reportProgress(account, report) } }
    }

    private fun playNext() {
        val next = nextEpisodeId ?: return
        report(PlaybackEvent.STOP)
        ui.hasNext = false
        lifecycleScope.launch { load(next, 0L) }
    }

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val down = event.action == KeyEvent.ACTION_DOWN
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
            KeyEvent.KEYCODE_BACK -> {
                // Close the panel, then hide the controls, then leave.
                if (ui.tracksOpen) {
                    if (down) ui.tracksOpen = false
                    return true
                }
                if (ui.visible && ui.playing) {
                    if (down) ui.visible = false
                    return true
                }
            }
        }
        if (!ui.visible && ui.nearEnd && (event.keyCode == KeyEvent.KEYCODE_DPAD_CENTER || event.keyCode == KeyEvent.KEYCODE_ENTER)) {
            // The "Next episode" prompt has the focus: OK goes to it.
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

    // Another title started while this one plays (e.g. from the app behind a PiP window).
    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        val acct = intent.getStringExtra(EXTRA_ACCOUNT) ?: return
        val id = intent.getStringExtra(EXTRA_ITEM) ?: return
        if (acct == account && id == itemId) return
        report(PlaybackEvent.STOP)
        account = acct
        nextEpisodeId = null
        prefetchedNext = false
        ui.hasNext = false
        lifecycleScope.launch { load(id, intent.getLongExtra(EXTRA_START, 0L)) }
    }

    /** Show (or movie) name, then "S1 · E2 · Episode" or the year. Works offline. */
    private fun showTitle(id: String) {
        val item = core.cachedItem(account, id)?.item
        if (item != null) {
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
            return
        }
        // Not cached: the download's title ("Show · S1 · E2 · Episode").
        val saved = runCatching { core.downloads() }.getOrNull()?.firstOrNull { it.accountId == account && it.itemId == id }?.title ?: return
        ui.title = saved.substringBefore(" · ")
        ui.subtitle = saved.substringAfter(" · ", "")
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
        report(PlaybackEvent.STOP)
        progressJob?.cancel()
        player?.release()
        player = null
        window.attributes = window.attributes.apply { preferredDisplayModeId = originalModeId }
        finish()
    }

    companion object {
        private const val EXTRA_ACCOUNT = "account"
        private const val EXTRA_ITEM = "item"
        private const val EXTRA_START = "start"

        fun intent(
            ctx: Context,
            account: String,
            itemId: String,
            startMs: Long,
        ) = Intent(ctx, PlayerActivity::class.java)
            .putExtra(EXTRA_ACCOUNT, account)
            .putExtra(EXTRA_ITEM, itemId)
            .putExtra(EXTRA_START, startMs)
    }
}
