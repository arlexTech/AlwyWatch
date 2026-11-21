// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.player

import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.Player
import androidx.media3.common.TrackSelectionOverride
import androidx.media3.common.Tracks
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.TrackInfo
import be.alwyapps.watch.core.TrackKind
import be.alwyapps.watch.core.TrackPref
import be.alwyapps.watch.core.TrackRules
import be.alwyapps.watch.core.chooseTracks
import be.alwyapps.watch.core.media.Item

/**
 * Audio and subtitle choices, like Plex: one per movie or per show (the episodes follow, by the
 * track's title and position, see the core's `tracks` module), picked on the title page or in
 * the player; otherwise the languages and subtitle mode of the settings. This file only maps
 * Media3 tracks to the core's neutral track list and back.
 */
object TrackPrefs {
    /** Episodes share their show's choice. */
    fun titleKey(item: Item): String =
        be.alwyapps.watch.core
            .trackTitleKey(item)

    fun get(
        core: AlwyWatch,
        account: String,
        titleKey: String,
    ): TrackPref? = runCatching { core.trackPref(account, titleKey) }.getOrNull()

    fun set(
        core: AlwyWatch,
        account: String,
        titleKey: String,
        pref: TrackPref,
    ) {
        runCatching { core.setTrackPref(account, titleKey, pref) }
    }

    /** The version (media source) chosen for an item, if any. */
    fun version(
        core: AlwyWatch,
        account: String,
        itemId: String,
    ): String? = runCatching { core.getSetting("version.$account.$itemId") }.getOrNull()?.ifEmpty { null }

    fun setVersion(
        core: AlwyWatch,
        account: String,
        itemId: String,
        sourceId: String,
    ) {
        runCatching { core.setSetting("version.$account.$itemId", sourceId) }
    }
}

/** Any language code ("fr", "fre", "fra", "fr-FR") → ISO 639-2/T ("fra"); null when unknown. */
fun lang3(code: String?): String? =
    be.alwyapps.watch.core
        .lang3(code)

/** A language's name in the device language ("French"). */
fun languageName(code: String?): String? {
    val c = lang3(code) ?: return null
    @Suppress("DEPRECATION")
    return java.util
        .Locale(c)
        .displayLanguage
        .ifEmpty { c }
        .replaceFirstChar { it.uppercase() }
}

private fun isForced(f: Format) = f.selectionFlags and C.SELECTION_FLAG_FORCED != 0

/** A player's playable audio and text tracks, in file order, numbered for the core. */
class PlayerTracks(
    tracks: Tracks,
) {
    val refs = mutableListOf<Pair<Tracks.Group, Int>>()
    val infos = mutableListOf<TrackInfo>()

    init {
        for (g in tracks.groups) {
            val kind =
                when (g.type) {
                    C.TRACK_TYPE_AUDIO -> TrackKind.AUDIO
                    C.TRACK_TYPE_TEXT -> TrackKind.SUBTITLE
                    else -> continue
                }
            for (i in 0 until g.length) {
                if (!g.isTrackSupported(i)) continue
                val f = g.getTrackFormat(i)
                infos += TrackInfo(refs.size.toUInt(), kind, f.language, f.label, isForced(f))
                refs += g to i
            }
        }
    }

    fun idOf(format: Format): UInt? =
        refs
            .indexOfFirst { (g, i) -> g.getTrackFormat(i).let { it.id == format.id && it.label == format.label && it.language == format.language } }
            .takeIf { it >= 0 }
            ?.toUInt()
}

/**
 * Picks audio and subtitles once the file's tracks are known: the title's choice, else the
 * settings (audio language; subtitles automatic/forced/always/off in the subtitle language).
 */
fun applyTrackPrefs(
    p: Player,
    pref: TrackPref?,
    settings: PlayerSettings,
) {
    val tracks = PlayerTracks(p.currentTracks)
    val default =
        tracks.refs
            .indexOfFirst { (g, i) -> g.type == C.TRACK_TYPE_AUDIO && g.isTrackSelected(i) }
            .takeIf { it >= 0 }
            ?.toUInt()
    val rules =
        TrackRules(
            settings.audioLanguage,
            settings.subtitleMode,
            settings.subtitleLanguage.ifEmpty {
                java.util.Locale
                    .getDefault()
                    .language
            },
            default,
        )
    val choice = chooseTracks(pref, rules, tracks.infos)
    val b = p.trackSelectionParameters.buildUpon()
    choice.audio?.let { id ->
        val (g, i) = tracks.refs[id.toInt()]
        b.setOverrideForType(TrackSelectionOverride(g.mediaTrackGroup, i))
    }
    val sub = choice.subtitle
    if (sub == null) {
        b.clearOverridesOfType(C.TRACK_TYPE_TEXT)
        b.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
    } else {
        val (g, i) = tracks.refs[sub.toInt()]
        b.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, false)
        b.setOverrideForType(TrackSelectionOverride(g.mediaTrackGroup, i))
    }
    p.trackSelectionParameters = b.build()
}
