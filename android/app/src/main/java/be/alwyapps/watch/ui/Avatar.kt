// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import android.content.Context
import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.keep.AvatarPreset
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.withContext
import java.io.File
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin
import be.alwyapps.watch.core.keep.Avatar as AvatarInfo

/** The twelve built-in avatars (from the core, which holds the table of docs/keep-protocol.md). */
object AvatarPresets {
    val all: List<AvatarPreset> by lazy {
        runCatching {
            be.alwyapps.watch.core
                .avatarPresets()
        }.getOrDefault(emptyList())
    }

    fun of(id: String?): AvatarPreset? = id?.let { i -> all.firstOrNull { it.id == i } }
}

/** `#RRGGBB` → color (grey when it isn't one). */
fun hexColor(hex: String): Color = runCatching { Color(0xFF000000 or hex.removePrefix("#").toLong(16)) }.getOrDefault(Color.Gray)

/**
 * A person's round avatar: their uploaded picture, a built-in one, or the default (the initial
 * of their name on a colour derived from it). While a picture loads, or if it can't, the default.
 */
@Composable
fun Avatar(
    avatar: AvatarInfo,
    name: String,
    userKey: String,
    size: Dp,
    modifier: Modifier = Modifier,
) {
    // No user key (a room's host in a list): their picture can't be fetched, the default shows.
    val version = avatar.customVersion?.takeIf { userKey.isNotEmpty() }
    if (version != null) {
        val px = with(LocalDensity.current) { size.roundToPx() }
        val ctx = LocalContext.current
        val core = LocalCore.current
        var bitmap by remember(userKey, version) { mutableStateOf(AvatarImages.cached(userKey, version, px)) }
        if (bitmap == null) {
            LaunchedEffect(userKey, version) { bitmap = AvatarImages.load(ctx, core, userKey, version, px) }
        }
        val b = bitmap
        if (b != null) {
            Image(b, name, modifier.size(size).clip(CircleShape), contentScale = ContentScale.Crop)
            return
        }
    }
    val preset = AvatarPresets.of(avatar.preset)
    if (preset != null) PresetAvatar(preset, size, modifier) else DefaultAvatar(name, size, modifier)
}

/** The default avatar: the name's initial in white on its colour. */
@Composable
fun DefaultAvatar(
    name: String,
    size: Dp,
    modifier: Modifier = Modifier,
) {
    val color =
        remember(name) {
            hexColor(
                be.alwyapps.watch.core
                    .avatarDefaultColor(name),
            )
        }
    val initial =
        remember(name) {
            be.alwyapps.watch.core
                .avatarInitial(name)
        }
    val sp = with(LocalDensity.current) { (size * 0.45f).toSp() }
    Box(modifier.size(size).clip(CircleShape).background(color), contentAlignment = Alignment.Center) {
        Text(initial, color = Color.White, fontSize = sp, fontWeight = FontWeight.Bold, maxLines = 1)
    }
}

/** A built-in avatar: a disc of its colour with its shape in white (geometry in docs/keep-protocol.md). */
@Composable
fun PresetAvatar(
    preset: AvatarPreset,
    size: Dp,
    modifier: Modifier = Modifier,
) {
    val bg = remember(preset.background) { hexColor(preset.background) }
    val fg = remember(preset.foreground) { hexColor(preset.foreground) }
    Canvas(modifier.size(size).clip(CircleShape)) {
        drawRect(bg)
        drawPresetShape(preset.shape, fg)
    }
}

/** Shapes in a unit box [-1, 1]², 1 unit = a quarter of the diameter, y down. */
private fun DrawScope.drawPresetShape(
    shape: String,
    color: Color,
) {
    val u = size.minDimension / 4f
    val c = center

    fun p(
        x: Float,
        y: Float,
    ) = Offset(c.x + x * u, c.y + y * u)

    fun rect(
        x0: Float,
        y0: Float,
        x1: Float,
        y1: Float,
    ) = drawRect(color, p(x0, y0), Size((x1 - x0) * u, (y1 - y0) * u))

    fun polygon(points: List<Pair<Float, Float>>) {
        val path = Path()
        points.forEachIndexed { i, (x, y) ->
            val o = p(x, y)
            if (i == 0) path.moveTo(o.x, o.y) else path.lineTo(o.x, o.y)
        }
        path.close()
        drawPath(path, color)
    }

    fun plus() {
        rect(-1f, -0.3f, 1f, 0.3f)
        rect(-0.3f, -1f, 0.3f, 1f)
    }
    when (shape) {
        "circle" -> drawCircle(color, u, c)
        "ring" -> drawCircle(color, 0.75f * u, c, style = Stroke(0.5f * u))
        "square" -> rect(-0.85f, -0.85f, 0.85f, 0.85f)
        "diamond" -> polygon(listOf(0f to -1f, 1f to 0f, 0f to 1f, -1f to 0f))
        "triangle" -> polygon(listOf(0f to -1f, 0.95f to 0.7f, -0.95f to 0.7f))
        "plus" -> plus()
        "cross" -> rotate(45f, c) { plus() }
        "star" ->
            polygon(
                (0 until 10).map { k ->
                    val r = if (k % 2 == 0) 1f else 0.45f
                    val a = (-PI / 2 + k * PI / 5).toFloat()
                    r * cos(a) to r * sin(a)
                },
            )
        "hexagon" ->
            polygon(
                (0 until 6).map { k ->
                    val a = (-PI / 2 + k * PI / 3).toFloat()
                    cos(a) to sin(a)
                },
            )
        "bars" -> {
            rect(-1f, -0.8f, 1f, -0.4f)
            rect(-1f, -0.2f, 1f, 0.2f)
            rect(-1f, 0.4f, 1f, 0.8f)
        }
        "half" -> drawArc(color, 180f, 180f, true, p(-1f, 0.25f - 1f), Size(2 * u, 2 * u))
        "dots" ->
            for (x in listOf(-0.5f, 0.5f)) {
                for (y in listOf(-0.5f, 0.5f)) drawCircle(color, 0.35f * u, p(x, y))
            }
    }
}

/** A few overlapping avatars (who is in a room), then "+n". */
@Composable
fun AvatarStack(
    people: List<Triple<AvatarInfo, String, String>>,
    size: Dp,
    modifier: Modifier = Modifier,
    max: Int = 4,
) {
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(-size / 3), verticalAlignment = Alignment.CenterVertically) {
        people.take(max).forEach { (avatar, name, key) ->
            Box(Modifier.size(size + 4.dp).clip(CircleShape).background(Background), contentAlignment = Alignment.Center) {
                Avatar(avatar, name, key, size)
            }
        }
        if (people.size > max) {
            Text(
                "+${people.size - max}",
                Modifier.offset(x = size / 3 + 4.dp),
                color = Muted,
                style = androidx.compose.material3.MaterialTheme.typography.labelMedium,
            )
        }
    }
}

/**
 * Uploaded avatars: fetched with the AlwyWatch sign-in headers (they aren't public), kept on
 * disk by (user, version) and never revalidated (a new picture is a new version), decoded at
 * the shown size. Failures are not retried until the app restarts.
 */
object AvatarImages {
    private val memory = LruCache<String, ImageBitmap>(48)
    private val failed = ConcurrentHashMap.newKeySet<String>()
    private val loading = ConcurrentHashMap<String, CompletableDeferred<File?>>()

    @OptIn(ExperimentalCoroutinesApi::class)
    private val io = Dispatchers.IO.limitedParallelism(2)

    private fun bucket(px: Int) = Images.bucket(px.coerceAtLeast(1)).coerceAtMost(512)

    fun cached(
        userKey: String,
        version: UInt,
        px: Int,
    ): ImageBitmap? = memory.get("$userKey|$version|${bucket(px)}")

    /** Stores a picture this device just uploaded (no download needed to show it). */
    fun put(
        ctx: Context,
        userKey: String,
        version: UInt,
        bytes: ByteArray,
    ) {
        runCatching {
            val f = file(ctx, userKey, version)
            f.parentFile?.mkdirs()
            f.writeBytes(bytes)
            prune(f)
        }
    }

    private fun file(
        ctx: Context,
        userKey: String,
        version: UInt,
    ): File {
        val digest =
            java.security.MessageDigest
                .getInstance("SHA-256")
                .digest(userKey.toByteArray())
                .take(12)
                .joinToString("") { "%02x".format(it) }
        return File(File(ctx.cacheDir, "avatars"), "${digest}_$version")
    }

    /** Older versions of the same person's picture. */
    private fun prune(keep: File) {
        val prefix = keep.name.substringBefore('_') + "_"
        keep.parentFile?.listFiles()?.forEach { if (it.name.startsWith(prefix) && it.name != keep.name) it.delete() }
    }

    suspend fun load(
        ctx: Context,
        core: AlwyWatch,
        userKey: String,
        version: UInt,
        px: Int,
    ): ImageBitmap? {
        val size = bucket(px)
        val key = "$userKey|$version|$size"
        memory.get(key)?.let { return it }
        val fileKey = "$userKey|$version"
        if (fileKey in failed) return null
        val f = file(ctx, userKey, version)
        val path =
            if (f.exists()) {
                f
            } else {
                val mine = CompletableDeferred<File?>()
                val other = loading.putIfAbsent(fileKey, mine)
                if (other != null) {
                    other.await()
                } else {
                    val got = withContext(io) { runCatching { fetch(core, userKey, version, f) }.getOrNull() }
                    loading.remove(fileKey)
                    mine.complete(got)
                    got
                }
            }
        if (path == null) {
            failed += fileKey
            return null
        }
        return withContext(io) {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(path.path, bounds)
            var sample = 1
            while (bounds.outWidth / (sample * 2) >= size) sample *= 2
            BitmapFactory
                .decodeFile(path.path, BitmapFactory.Options().apply { inSampleSize = sample })
                ?.asImageBitmap()
                ?.also { memory.put(key, it) }
        }
    }

    /** Downloads the picture (sign-in headers from the core; never logged). */
    private fun fetch(
        core: AlwyWatch,
        userKey: String,
        version: UInt,
        into: File,
    ): File? {
        val req = core.keepAvatarRequest(userKey, version)
        val conn = java.net.URL(req.url).openConnection() as java.net.HttpURLConnection
        try {
            conn.connectTimeout = 15_000
            conn.readTimeout = 20_000
            req.headers.forEach { conn.setRequestProperty(it.name, it.value) }
            if (conn.responseCode != 200) return null
            // The server keeps them under 512 KiB: anything much bigger is not an avatar.
            val bytes = conn.inputStream.use { it.readNBytesCompat(1 shl 20) } ?: return null
            into.parentFile?.mkdirs()
            val tmp = File(into.parentFile, into.name + ".part")
            tmp.writeBytes(bytes)
            if (!tmp.renameTo(into)) return null
            prune(into)
            return into
        } finally {
            conn.disconnect()
        }
    }

    private fun java.io.InputStream.readNBytesCompat(max: Int): ByteArray? {
        val out = java.io.ByteArrayOutputStream()
        val buf = ByteArray(16 * 1024)
        while (true) {
            val n = read(buf)
            if (n < 0) break
            out.write(buf, 0, n)
            if (out.size() > max) return null
        }
        return out.toByteArray()
    }
}

/** A ring around an avatar: the choice in a grid. */
@Composable
fun AvatarRing(
    selected: Boolean,
    size: Dp,
    content: @Composable () -> Unit,
) {
    Box(
        Modifier
            .size(size + 10.dp)
            .then(if (selected) Modifier.background(Accent, CircleShape) else Modifier),
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.size(size + 4.dp).clip(CircleShape).background(Background), contentAlignment = Alignment.Center) {
            Box(Modifier.size(size), contentAlignment = Alignment.Center) { content() }
        }
    }
}
