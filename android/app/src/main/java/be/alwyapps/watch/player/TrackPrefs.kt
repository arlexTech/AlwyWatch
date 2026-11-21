// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.player

import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.Player
import androidx.media3.common.TrackSelectionOverride
import androidx.media3.common.Tracks
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.media.Item
import be.alwyapps.watch.core.media.ItemKind

/**
 * Audio and subtitle choices, like Plex: one per movie or per show (the episodes follow), picked
 * on the title page or in the player; otherwise the languages and subtitle mode of the settings.
 */
data class TrackPref(
    /** Audio language (ISO 639-2/T) and track title when several share it. */
    val audio: String? = null,
    val audioTitle: String? = null,
    /** Null: the settings decide; "off"; or a language, with [forced] for signs-only tracks. */
    val subs: String? = null,
    val forced: Boolean = false,
    /**
     * Position among the tracks of the same language (and kind, for subtitles), and the subtitle's
     * title: a season or a show is usually packed by one person with the same naming and order,
     * so "the second French track, titled VFQ" finds the same track in every episode.
     */
    val audioNth: Int? = null,
    val subsTitle: String? = null,
    val subsNth: Int? = null,
) {
    fun encode() =
        listOf(
            audio.orEmpty(),
            audioTitle.orEmpty(),
            subs.orEmpty(),
            if (forced) "1" else "",
            audioNth?.toString().orEmpty(),
            subsTitle.orEmpty(),
            subsNth?.toString().orEmpty(),
        ).joinToString("\u001f")

    companion object {
        fun decode(s: String?): TrackPref? {
            val p = s?.split('\u001f') ?: return null
            if (p.size < 4) return null
            return TrackPref(
                p[0].ifEmpty { null },
                p[1].ifEmpty { null },
                p[2].ifEmpty { null },
                p[3] == "1",
                p.getOrNull(4)?.toIntOrNull(),
                p.getOrNull(5)?.ifEmpty { null },
                p.getOrNull(6)?.toIntOrNull(),
            )
        }
    }
}

/**
 * The track like the remembered one among [same] (the tracks of its language and kind, in file
 * order): same title, else same position, else the first.
 */
fun <T> likeRemembered(
    same: List<T>,
    title: String?,
    nth: Int?,
    titleOf: (T) -> String?,
): T? {
    val t = title?.trim()?.lowercase()
    return same.firstOrNull { t != null && titleOf(it)?.trim()?.lowercase() == t }
        ?: nth?.let { same.getOrNull(it) }
        ?: same.firstOrNull()
}

object TrackPrefs {
    /** Episodes share their show's choice. */
    fun titleKey(item: Item): String = if (item.kind == ItemKind.EPISODE) item.seriesId ?: item.id else item.id

    fun get(
        core: AlwyWatch,
        account: String,
        titleKey: String,
    ): TrackPref? = TrackPref.decode(runCatching { core.getSetting("tracks.$account.$titleKey") }.getOrNull())

    fun set(
        core: AlwyWatch,
        account: String,
        titleKey: String,
        pref: TrackPref,
    ) {
        runCatching { core.setSetting("tracks.$account.$titleKey", pref.encode()) }
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

/** Bibliographic ISO 639-2 codes (what servers often send) → terminology codes (what Java uses). */
private val BIBLIOGRAPHIC =
    mapOf(
        "fre" to "fra",
        "ger" to "deu",
        "dut" to "nld",
        "chi" to "zho",
        "cze" to "ces",
        "gre" to "ell",
        "per" to "fas",
        "rum" to "ron",
        "slo" to "slk",
        "alb" to "sqi",
        "arm" to "hye",
        "baq" to "eus",
        "bur" to "mya",
        "geo" to "kat",
        "ice" to "isl",
        "mac" to "mkd",
        "mao" to "mri",
        "may" to "msa",
        "tib" to "bod",
        "wel" to "cym",
    )

/** Any language code ("fr", "fre", "fra", "fr-FR") → ISO 639-2/T ("fra"); null when unknown. */
fun lang3(code: String?): String? {
    val c =
        code
            ?.trim()
            ?.lowercase()
            ?.substringBefore('-')
            ?.substringBefore('_')
            ?.takeIf { it.isNotEmpty() && it != "und" && it != "unknown" } ?: return null
    BIBLIOGRAPHIC[c]?.let { return it }
    return runCatching {
        @Suppress("DEPRECATION")
        java.util.Locale(c).isO3Language
    }.getOrNull()?.ifEmpty { null } ?: c
}

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

/**
 * Picks audio and subtitles once the file's tracks are known: the title's choice, else the
 * settings (audio language; subtitles automatic/forced/always/off in the subtitle language).
 */
fun applyTrackPrefs(
    p: Player,
    pref: TrackPref?,
    settings: PlayerSettings,
) {
    val tracks = p.currentTracks
    val b = p.trackSelectionParameters.buildUpon()

    /** Playable tracks of a type, in file order. */
    fun all(type: Int): List<Pair<Tracks.Group, Int>> =
        tracks.groups.filter { it.type == type }.flatMap { g -> (0 until g.length).filter { g.isTrackSupported(it) }.map { g to it } }

    fun Pair<Tracks.Group, Int>.format() = first.getTrackFormat(second)

    fun find(
        type: Int,
        match: (Format) -> Boolean,
    ): Pair<Tracks.Group, Int>? = all(type).firstOrNull { match(it.format()) }

    // Audio.
    val wantAudio = pref?.audio ?: lang3(settings.audioLanguage)
    var audioLang: String? = null
    if (wantAudio != null) {
        val same = all(C.TRACK_TYPE_AUDIO).filter { lang3(it.format().language) == wantAudio }
        val hit =
            if (pref?.audio != null) likeRemembered(same, pref.audioTitle, pref.audioNth) { it.format().label } else same.firstOrNull()
        if (hit != null) {
            b.setOverrideForType(TrackSelectionOverride(hit.first.mediaTrackGroup, hit.second))
            audioLang = wantAudio
        }
    }
    if (audioLang == null) {
        audioLang =
            tracks.groups
                .firstOrNull { it.type == C.TRACK_TYPE_AUDIO && it.isSelected }
                ?.let { g -> (0 until g.length).firstOrNull { g.isTrackSelected(it) }?.let { lang3(g.getTrackFormat(it).language) } }
    }

    // Subtitles.
    val subLang =
        lang3(
            settings.subtitleLanguage.ifEmpty {
                java.util.Locale
                    .getDefault()
                    .language
            },
        )

    fun pick(
        lang: String?,
        forcedOnly: Boolean,
    ): Boolean {
        val hit =
            if (forcedOnly) {
                find(C.TRACK_TYPE_TEXT) { isForced(it) && (lang == null || lang3(it.language) == lang) }
            } else {
                find(C.TRACK_TYPE_TEXT) { !isForced(it) && lang3(it.language) == lang }
                    ?: find(C.TRACK_TYPE_TEXT) { lang3(it.language) == lang }
            }
        if (hit == null) return false
        b.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, false)
        b.setOverrideForType(TrackSelectionOverride(hit.first.mediaTrackGroup, hit.second))
        return true
    }

    fun off() {
        b.clearOverridesOfType(C.TRACK_TYPE_TEXT)
        b.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, true)
    }

    // The remembered subtitle: same language and kind, then same title or position.
    fun pickRemembered(pref: TrackPref): Boolean {
        val same = all(C.TRACK_TYPE_TEXT).filter { lang3(it.format().language) == pref.subs && isForced(it.format()) == pref.forced }
        val hit = likeRemembered(same, pref.subsTitle, pref.subsNth) { it.format().label } ?: return pick(pref.subs, pref.forced)
        b.setTrackTypeDisabled(C.TRACK_TYPE_TEXT, false)
        b.setOverrideForType(TrackSelectionOverride(hit.first.mediaTrackGroup, hit.second))
        return true
    }

    val chosen = pref?.subs
    when {
        chosen == "off" -> off()
        chosen != null -> if (!pickRemembered(pref)) off()
        settings.subtitleMode == "off" -> off()
        settings.subtitleMode == "always" -> if (!pick(subLang, false)) off()
        settings.subtitleMode == "forced" -> if (!pick(audioLang, true)) off()
        // Automatic: the audio is in another language than ours → subtitles in ours; else only
        // forced ones (signs, foreign dialogue).
        audioLang != null && subLang != null && audioLang != subLang -> if (!pick(subLang, false) && !pick(audioLang, true)) off()
        else -> if (!pick(audioLang, true)) off()
    }
    p.trackSelectionParameters = b.build()
}
