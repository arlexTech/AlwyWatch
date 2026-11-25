// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.player

import android.net.Uri
import androidx.annotation.OptIn
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.TransferListener
import be.alwyapps.watch.core.media.BytePatch

/**
 * Reads [uri] through [upstream] with [patches] shown in place of the file's own bytes (same
 * length: nothing shifts). A Matroska original with its track list at the end plays this way,
 * the list moved into the padding before the picture (see the core's `mkv::fix`). Other URIs
 * (side-loaded subtitles) pass through untouched.
 */
@OptIn(UnstableApi::class)
class PatchedDataSource(
    private val upstream: DataSource,
    private val uri: String,
    private val patches: List<BytePatch>,
) : DataSource {
    private var position = 0L
    private var active = false

    override fun addTransferListener(transferListener: TransferListener) = upstream.addTransferListener(transferListener)

    override fun open(dataSpec: DataSpec): Long {
        active = dataSpec.uri.toString() == uri
        position = dataSpec.position
        return upstream.open(dataSpec)
    }

    override fun read(
        buffer: ByteArray,
        offset: Int,
        length: Int,
    ): Int {
        val n = upstream.read(buffer, offset, length)
        if (n > 0 && active) {
            val start = position
            val end = position + n
            for (p in patches) {
                val pStart = p.offset.toLong()
                val pEnd = pStart + p.bytes.size
                val from = maxOf(start, pStart)
                val to = minOf(end, pEnd)
                if (from < to) {
                    System.arraycopy(p.bytes, (from - pStart).toInt(), buffer, offset + (from - start).toInt(), (to - from).toInt())
                }
            }
        }
        if (n > 0) position += n
        return n
    }

    override fun getUri(): Uri? = upstream.uri

    override fun getResponseHeaders(): Map<String, List<String>> = upstream.responseHeaders

    override fun close() = upstream.close()

    class Factory(
        private val upstream: DataSource.Factory,
        private val uri: String,
        private val patches: List<BytePatch>,
    ) : DataSource.Factory {
        override fun createDataSource(): DataSource = PatchedDataSource(upstream.createDataSource(), uri, patches)
    }
}
