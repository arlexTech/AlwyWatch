// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.player

import android.content.Context
import android.hardware.display.DisplayManager
import android.media.MediaCodecInfo.CodecProfileLevel
import android.media.MediaCodecList
import android.view.Display
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.media.DeviceProfile
import be.alwyapps.watch.core.media.HdrKind
import be.alwyapps.watch.core.media.MediaSource
import be.alwyapps.watch.core.media.StreamKind

/**
 * What this device can play without the server touching the file.
 * Video: decoders from MediaCodecList (hardware only for HEVC, VP9 and AV1). Audio: platform decoders plus the
 * bundled FFmpeg decoder (DTS, DTS-HD, TrueHD, AC3, E-AC3...), and passthrough when an HDMI
 * receiver supports it (ExoPlayer negotiates that by itself). Computed once, then cached.
 */
object DeviceProfiles {
    @Volatile private var cached: DeviceProfile? = null

    fun get(ctx: Context): DeviceProfile = cached ?: build(ctx.applicationContext).also { cached = it }

    /** [get] off the main thread (the codec list takes tens of ms the first time). */
    suspend fun load(ctx: Context): DeviceProfile = cached ?: kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) { get(ctx) }

    private val mimeToCodec =
        mapOf(
            "video/avc" to "h264",
            "video/hevc" to "hevc",
            "video/x-vnd.on2.vp9" to "vp9",
            "video/x-vnd.on2.vp8" to "vp8",
            "video/av01" to "av1",
            "video/mpeg2" to "mpeg2video",
            "video/mp4v-es" to "mpeg4",
            "video/wvc1" to "vc1",
            "video/x-ms-wmv" to "vc1",
        )

    private val HARDWARE_ONLY = setOf("hevc", "vp9", "av1")

    private fun isHardware(info: android.media.MediaCodecInfo): Boolean =
        if (android.os.Build.VERSION.SDK_INT >= 29) {
            info.isHardwareAccelerated
        } else {
            val n = info.name.lowercase()
            !(n.startsWith("omx.google.") || n.startsWith("c2.android.") || n.startsWith("c2.google."))
        }

    private fun build(ctx: Context): DeviceProfile {
        val video = linkedSetOf<String>()
        var maxW = 0
        var maxH = 0
        var dolbyVisionDecoder = false
        val tenBit = linkedSetOf<String>()
        for (info in MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos) {
            if (info.isEncoder) continue
            for (type in info.supportedTypes) {
                val t = type.lowercase()
                if (t == "video/dolby-vision") dolbyVisionDecoder = true
                val codec = mimeToCodec[t] ?: continue
                // Modern codecs only count with a hardware decoder: Android ships software
                // ones (AV1, VP9, HEVC) that manage a few frames per second at 1080p on a
                // TV stick. Without the chip, the server converts instead.
                if (codec in HARDWARE_ONLY && !isHardware(info)) continue
                video += codec
                runCatching {
                    val levels = info.getCapabilitiesForType(type).profileLevels.map { it.profile }
                    val ten =
                        when (codec) {
                            "hevc" -> levels.any { it == CodecProfileLevel.HEVCProfileMain10 || it == CodecProfileLevel.HEVCProfileMain10HDR10 }
                            "h264" -> levels.any { it == CodecProfileLevel.AVCProfileHigh10 }
                            "vp9" -> levels.any { it == CodecProfileLevel.VP9Profile2 || it == CodecProfileLevel.VP9Profile2HDR }
                            "av1" -> levels.any { it == CodecProfileLevel.AV1ProfileMain10 || it == CodecProfileLevel.AV1ProfileMain10HDR10 }
                            else -> false
                        }
                    if (ten) tenBit += codec
                }
                runCatching {
                    val caps = info.getCapabilitiesForType(type).videoCapabilities ?: return@runCatching
                    maxW = maxOf(maxW, caps.supportedWidths.upper)
                    maxH = maxOf(maxH, caps.supportedHeights.upper)
                }
            }
        }
        val hdr = mutableListOf(HdrKind.SDR)
        val display = ctx.getSystemService(DisplayManager::class.java)?.getDisplay(Display.DEFAULT_DISPLAY)
        @Suppress("DEPRECATION")
        display?.hdrCapabilities?.supportedHdrTypes?.forEach {
            when (it) {
                Display.HdrCapabilities.HDR_TYPE_HDR10 -> hdr += HdrKind.HDR10
                Display.HdrCapabilities.HDR_TYPE_HLG -> hdr += HdrKind.HLG
                Display.HdrCapabilities.HDR_TYPE_HDR10_PLUS -> hdr += HdrKind.HDR10_PLUS
                Display.HdrCapabilities.HDR_TYPE_DOLBY_VISION -> if (dolbyVisionDecoder) hdr += HdrKind.DOLBY_VISION
            }
        }
        return DeviceProfile(
            maxBitrate = 200_000_000uL,
            containers = listOf("mkv", "matroska", "webm", "mp4", "m4v", "mov", "ts", "mpegts", "m2ts", "ogg", "flv"),
            videoCodecs = video.toList(),
            video10bitCodecs = tenBit.toList(),
            audioCodecs =
                listOf(
                    "aac",
                    "mp3",
                    "mp2",
                    "opus",
                    "vorbis",
                    "flac",
                    "alac",
                    "pcm",
                    "pcm_s16le",
                    "pcm_s24le",
                    // Bundled FFmpeg decoder (or passthrough when a receiver supports it):
                    "ac3",
                    "eac3",
                    "dts",
                    "dca",
                    "truehd",
                    "mlp",
                ),
            subtitleCodecs =
                listOf(
                    "srt",
                    "subrip",
                    "ass",
                    "ssa",
                    "vtt",
                    "webvtt",
                    "ttml",
                    "pgssub",
                    "pgs",
                    "dvdsub",
                    "dvd_subtitle",
                    "dvbsub",
                ),
            maxWidth = maxW.takeIf { it > 0 }?.toUInt(),
            maxHeight = maxH.takeIf { it > 0 }?.toUInt(),
            hdr = hdr,
        )
    }

    /** True when the original file can be played as-is on this device. */
    fun canDirectPlay(
        source: MediaSource,
        profile: DeviceProfile,
    ): Boolean {
        val video = source.streams.firstOrNull { it.kind == StreamKind.VIDEO } ?: return true
        val container =
            source.container
                ?.lowercase()
                ?.split(',')
                ?.firstOrNull()
        val containerOk = container == null || container in profile.containers
        val audioOk =
            source.streams.none { it.kind == StreamKind.AUDIO } ||
                source.streams.any { it.kind == StreamKind.AUDIO && it.codec in profile.audioCodecs }
        val depthOk = (video.bitDepth ?: 8u) <= 8u || video.codec in profile.video10bitCodecs
        val sizeOk =
            (profile.maxWidth == null || (video.width ?: 0u) <= profile.maxWidth!!) &&
                (profile.maxHeight == null || (video.height ?: 0u) <= profile.maxHeight!!)
        return video.codec in profile.videoCodecs && depthOk && sizeOk && containerOk && audioOk
    }
}

/** Player preferences stored in the core settings. */
class PlayerSettings(
    private val core: AlwyWatch,
    private val isTv: Boolean = true,
) {
    var tunneling: Boolean
        get() = core.getSetting("player.tunneling") == "1"
        set(v) = core.setSetting("player.tunneling", if (v) "1" else "0")

    /** Switch the TV display to the video's frame rate (no judder on 24p films). */
    var matchFrameRate: Boolean
        get() = core.getSetting("player.match_rate")?.let { it == "1" } ?: isTv
        set(v) = core.setSetting("player.match_rate", if (v) "1" else "0")

    /** Jump over intros and recaps without asking (when the server marked them). */
    var autoSkipIntro: Boolean
        get() = core.getSetting("player.auto_skip_intro") == "1"
        set(v) = core.setSetting("player.auto_skip_intro", if (v) "1" else "0")

    /** When the credits start, play the next episode after a short countdown. */
    var autoPlayNext: Boolean
        get() = core.getSetting("player.auto_next") != "0"
        set(v) = core.setSetting("player.auto_next", if (v) "1" else "0")

    private fun str(
        key: String,
        default: String,
    ) = core.getSetting(key)?.takeIf { it.isNotEmpty() } ?: default

    /** Preferred audio language (ISO 639-2, "fre"), or "" for each file's original/default track. */
    var audioLanguage: String
        get() = core.getSetting("player.audio_lang").orEmpty()
        set(v) = core.setSetting("player.audio_lang", v)

    /**
     * Subtitles: "auto" (when the audio is not in the subtitle language, like Plex), "forced"
     * (signs and foreign parts only), "always", "off".
     */
    var subtitleMode: String
        get() = str("player.sub_mode", "auto")
        set(v) = core.setSetting("player.sub_mode", v)

    /** Subtitle language (ISO 639-2), or "" for the device language. */
    var subtitleLanguage: String
        get() = core.getSetting("player.sub_lang").orEmpty()
        set(v) = core.setSetting("player.sub_lang", v)

    /** Subtitle look: size (small, normal, large, huge), colour, background box, raised position. */
    var subtitleSize: String
        get() = str("player.sub_size", "normal")
        set(v) = core.setSetting("player.sub_size", v)

    var subtitleColor: String
        get() = str("player.sub_color", "white")
        set(v) = core.setSetting("player.sub_color", v)

    var subtitleBackground: Boolean
        get() = core.getSetting("player.sub_bg") == "1"
        set(v) = core.setSetting("player.sub_bg", if (v) "1" else "0")

    var subtitleRaised: Boolean
        get() = core.getSetting("player.sub_raised") == "1"
        set(v) = core.setSetting("player.sub_raised", if (v) "1" else "0")

    /** Seconds played again when resuming (to remember where you were). */
    var resumeRewindS: Int
        get() = core.getSetting("player.resume_rewind")?.toIntOrNull() ?: 0
        set(v) = core.setSetting("player.resume_rewind", v.toString())

    var seekBackS: Int
        get() = core.getSetting("player.seek_back")?.toIntOrNull() ?: 10
        set(v) = core.setSetting("player.seek_back", v.toString())

    var seekForwardS: Int
        get() = core.getSetting("player.seek_forward")?.toIntOrNull() ?: 30
        set(v) = core.setSetting("player.seek_forward", v.toString())

    /** Seconds of the credits countdown before the next episode. */
    var countdownS: Int
        get() = core.getSetting("player.countdown")?.toIntOrNull() ?: 10
        set(v) = core.setSetting("player.countdown", v.toString())

    /** "Are you still watching?" after this many hours without touching the remote (0: never). */
    var stillWatchingH: Int
        get() = core.getSetting("player.still_watching")?.toIntOrNull() ?: 2
        set(v) = core.setSetting("player.still_watching", v.toString())

    /** Show pages play the show's theme song quietly (like Plex). */
    var themeMusic: Boolean
        get() = core.getSetting("ui.theme_music")?.let { it == "1" } ?: isTv
        set(v) = core.setSetting("ui.theme_music", if (v) "1" else "0")
}
