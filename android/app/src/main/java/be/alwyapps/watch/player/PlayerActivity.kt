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
                setShowSubtitleButton(true)
                setShowNextButton(false)
                setShowPreviousButton(false)
                controllerShowTimeoutMs = 4000
                keepScreenOn = true
            }
        setContentView(view)
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
        view.requestFocus()
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
                val p = player ?: return
                if (!isPlaying && p.playbackState == Player.STATE_READY) report(PlaybackEvent.PAUSE)
            }

            override fun onPlaybackStateChanged(state: Int) {
                if (state == Player.STATE_ENDED) {
                    report(PlaybackEvent.STOP)
                    val next = nextEpisodeId
                    if (next != null) lifecycleScope.launch { load(next, 0L) } else finish()
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

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        // Any remote key wakes the controls first instead of acting blindly.
        if (event.action == KeyEvent.ACTION_DOWN &&
            !view.isControllerFullyVisible &&
            event.keyCode in
            setOf(KeyEvent.KEYCODE_DPAD_UP, KeyEvent.KEYCODE_DPAD_DOWN, KeyEvent.KEYCODE_DPAD_CENTER, KeyEvent.KEYCODE_ENTER)
        ) {
            view.showController()
            return true
        }
        return view.dispatchKeyEvent(event) || super.dispatchKeyEvent(event)
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
