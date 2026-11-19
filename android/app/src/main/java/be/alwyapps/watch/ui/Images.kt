// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Build
import android.util.LruCache
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import be.alwyapps.watch.core.AlwyWatch
import be.alwyapps.watch.core.media.ImageRef
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.withContext
import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.pow

/**
 * Minimal image pipeline (no image library): the Rust core downloads and caches files on disk,
 * this decodes them at the displayed size into hardware bitmaps and keeps a small memory LRU.
 */
object Images {
    private val widths = intArrayOf(160, 240, 320, 480, 720, 1080, 1920)

    fun bucket(px: Int): Int = widths.firstOrNull { it >= px } ?: widths.last()

    private val memory =
        object : LruCache<String, ImageBitmap>((Runtime.getRuntime().maxMemory() / 8).toInt()) {
            override fun sizeOf(
                key: String,
                value: ImageBitmap,
            ): Int = value.width * value.height * 4
        }

    @OptIn(ExperimentalCoroutinesApi::class)
    private val decoder = Dispatchers.IO.limitedParallelism(3)

    fun key(
        account: String,
        ref: ImageRef,
        px: Int,
    ) = "$account|${ref.path}|${bucket(px)}"

    fun cachedInMemory(key: String): ImageBitmap? = memory.get(key)

    suspend fun load(
        core: AlwyWatch,
        account: String,
        itemId: String,
        ref: ImageRef,
        px: Int,
    ): ImageBitmap? {
        val k = key(account, ref, px)
        memory.get(k)?.let { return it }
        val w = bucket(px).toUInt()
        val path =
            core.cachedImage(account, itemId, ref, w) ?: runCatching { core.image(account, itemId, ref, w) }.getOrNull() ?: return null
        return withContext(decoder) {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
            BitmapFactory.decodeFile(path, bounds)
            var sample = 1
            while (bounds.outWidth / (sample * 2) >= px) sample *= 2
            val opts =
                BitmapFactory.Options().apply {
                    inSampleSize = sample
                    // Hardware bitmaps live in GPU memory, not the Java heap.
                    if (Build.VERSION.SDK_INT >= 26) inPreferredConfig = Bitmap.Config.HARDWARE
                }
            BitmapFactory.decodeFile(path, opts)?.asImageBitmap()?.also { memory.put(k, it) }
        }
    }

    /** Same pipeline for plain public URLs (Seerr/TMDB posters). */
    suspend fun loadUrl(
        core: AlwyWatch,
        url: String,
        px: Int,
    ): ImageBitmap? {
        val k = "url|$url|${bucket(px)}"
        memory.get(k)?.let { return it }
        val path = core.cachedRemoteImage(url) ?: runCatching { core.remoteImage(url) }.getOrNull() ?: return null
        return withContext(decoder) {
            val opts =
                BitmapFactory.Options().apply {
                    if (Build.VERSION.SDK_INT >= 26) inPreferredConfig = Bitmap.Config.HARDWARE
                }
            BitmapFactory.decodeFile(path, opts)?.asImageBitmap()?.also { memory.put(k, it) }
        }
    }

    fun clearMemory() = memory.evictAll()
}

@Composable
fun UrlImage(
    url: String?,
    width: Dp,
    modifier: Modifier = Modifier,
) {
    val core = LocalCore.current
    val px = with(LocalDensity.current) { width.roundToPx() }
    var bitmap by remember(url) { mutableStateOf<ImageBitmap?>(null) }
    if (url != null && bitmap == null) {
        LaunchedEffect(url) { bitmap = Images.loadUrl(core, url, px) }
    }
    Box(modifier.background(SurfaceColor)) {
        bitmap?.let { Image(it, null, Modifier.matchParentSize(), contentScale = ContentScale.Crop) }
    }
}

/** Remote image with an instant BlurHash (or flat color) placeholder and no fade animation. */
@Composable
fun RemoteImage(
    itemId: String,
    ref: ImageRef?,
    width: Dp,
    modifier: Modifier = Modifier,
    contentScale: ContentScale = ContentScale.Crop,
    contentDescription: String? = null,
    background: androidx.compose.ui.graphics.Color = SurfaceColor,
    alignment: androidx.compose.ui.Alignment = androidx.compose.ui.Alignment.Center,
) {
    val core = LocalCore.current
    val account = LocalAccount.current
    val px = with(LocalDensity.current) { width.roundToPx() }
    var bitmap by remember(ref?.path, px) {
        mutableStateOf(ref?.let { Images.cachedInMemory(Images.key(account, it, px)) })
    }
    val placeholder = remember(ref?.blurhash) { ref?.blurhash?.let { BlurHash.decode(it, 24, 36) } }
    if (ref != null && bitmap == null) {
        LaunchedEffect(ref.path, px) { bitmap = Images.load(core, account, itemId, ref, px) }
    }
    // Logos (Fit) get no placeholder: a blurred block would show behind transparent art.
    val fit = contentScale == ContentScale.Fit
    Box(modifier.background(background)) {
        val img = bitmap ?: placeholder.takeIf { !fit }
        if (img != null) {
            Image(img, contentDescription, if (fit) Modifier else Modifier.matchParentSize(), contentScale = contentScale, alignment = alignment)
        }
    }
}

/** BlurHash decoder (https://blurha.sh), small enough not to need a library. */
object BlurHash {
    private const val CHARS = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~"

    private fun decode83(
        s: String,
        from: Int,
        to: Int,
    ): Int {
        var v = 0
        for (i in from until to) v = v * 83 + CHARS.indexOf(s[i])
        return v
    }

    private fun srgbToLinear(v: Int): Float {
        val x = v / 255f
        return if (x <= 0.04045f) x / 12.92f else ((x + 0.055f) / 1.055f).pow(2.4f)
    }

    private fun linearToSrgb(v: Float): Int {
        val x = v.coerceIn(0f, 1f)
        val s = if (x <= 0.0031308f) x * 12.92f else 1.055f * x.pow(1 / 2.4f) - 0.055f
        return (s * 255 + 0.5f).toInt().coerceIn(0, 255)
    }

    /** The image's average color (the hash's DC term): free ambient color for any artwork. */
    fun averageColor(hash: String?): androidx.compose.ui.graphics.Color? {
        if (hash == null || hash.length < 6) return null
        return runCatching {
            val v = decode83(hash, 2, 6)
            androidx.compose.ui.graphics
                .Color(0xFF000000.toInt() or v)
        }.getOrNull()
    }

    fun decode(
        hash: String,
        width: Int,
        height: Int,
    ): ImageBitmap? {
        if (hash.length < 6) return null
        return runCatching {
            val size = decode83(hash, 0, 1)
            val nx = size % 9 + 1
            val ny = size / 9 + 1
            if (hash.length != 4 + 2 * nx * ny) return null
            val maxAc = (decode83(hash, 1, 2) + 1) / 166f
            val colors =
                Array(nx * ny) { i ->
                    if (i == 0) {
                        val v = decode83(hash, 2, 6)
                        floatArrayOf(srgbToLinear(v shr 16), srgbToLinear((v shr 8) and 255), srgbToLinear(v and 255))
                    } else {
                        val v = decode83(hash, 4 + i * 2, 6 + i * 2)

                        // signPow((c - 9) / 9, 2) * maxAc
                        fun q(c: Int) = ((c - 9) / 9f).let { it * abs(it) } * maxAc
                        floatArrayOf(q(v / (19 * 19)), q((v / 19) % 19), q(v % 19))
                    }
                }
            val pixels = IntArray(width * height)
            for (y in 0 until height) {
                for (x in 0 until width) {
                    var r = 0f
                    var g = 0f
                    var b = 0f
                    for (j in 0 until ny) {
                        for (i in 0 until nx) {
                            val basis = (cos(PI * x * i / width) * cos(PI * y * j / height)).toFloat()
                            val c = colors[i + j * nx]
                            r += c[0] * basis
                            g += c[1] * basis
                            b += c[2] * basis
                        }
                    }
                    pixels[x + y * width] = (0xFF shl 24) or (linearToSrgb(r) shl 16) or (linearToSrgb(g) shl 8) or linearToSrgb(b)
                }
            }
            Bitmap.createBitmap(pixels, width, height, Bitmap.Config.ARGB_8888).asImageBitmap()
        }.getOrNull()
    }
}
