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
 * Video: hardware/platform decoders from MediaCodecList. Audio: platform decoders plus the
 * bundled FFmpeg decoder (DTS, DTS-HD, TrueHD, AC3, E-AC3...), and passthrough when an HDMI
 * receiver supports it (ExoPlayer negotiates that by itself). Computed once, then cached.
 */
object DeviceProfiles {
    @Volatile private var cached: DeviceProfile? = null

    fun get(ctx: Context): DeviceProfile = cached ?: build(ctx.applicationContext).also { cached = it }

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
}
