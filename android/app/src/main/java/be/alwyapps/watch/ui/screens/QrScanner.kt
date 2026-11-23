// SPDX-License-Identifier: AGPL-3.0-or-later
package be.alwyapps.watch.ui.screens

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.ImageFormat
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.media.ImageReader
import android.os.Handler
import android.os.HandlerThread
import android.util.Size
import android.view.Surface
import android.view.TextureView
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import be.alwyapps.watch.ui.ActionButton
import be.alwyapps.watch.ui.Background
import be.alwyapps.watch.ui.Danger
import be.alwyapps.watch.ui.Muted
import be.alwyapps.watch.ui.tr
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Full-screen camera that reads another device's "Sign in with a code" QR code and hands its
 * six digits to [onCode]. Android's own camera API and the core's QR reader: no camera library,
 * no Play Services. The caller has the CAMERA permission already.
 */
@Composable
fun QrScanner(
    onCode: (String) -> Unit,
    onClose: () -> Unit,
) {
    val ctx = LocalContext.current
    val found by rememberUpdatedState(onCode)
    var problem by remember { mutableStateOf<String?>(null) }
    val camera = remember { QrCamera(ctx) }
    DisposableEffect(Unit) {
        onDispose { camera.stop() }
    }
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Box(Modifier.fillMaxSize().background(Background), contentAlignment = Alignment.Center) {
            Column(
                Modifier.fillMaxWidth().padding(20.dp),
                verticalArrangement = Arrangement.spacedBy(16.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(tr("Point at the QR code on the other device"), style = MaterialTheme.typography.titleMedium)
                val size = camera.size
                AndroidView(
                    factory = { c ->
                        TextureView(c).apply {
                            surfaceTextureListener =
                                object : TextureView.SurfaceTextureListener {
                                    override fun onSurfaceTextureAvailable(
                                        st: SurfaceTexture,
                                        w: Int,
                                        h: Int,
                                    ) {
                                        camera.start(st, onCode = { code -> post { found(code) } }, onError = { post { problem = it } })
                                    }

                                    override fun onSurfaceTextureSizeChanged(
                                        st: SurfaceTexture,
                                        w: Int,
                                        h: Int,
                                    ) = Unit

                                    override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                                        camera.stop()
                                        return true
                                    }

                                    override fun onSurfaceTextureUpdated(st: SurfaceTexture) = Unit
                                }
                        }
                    },
                    // The sensor is landscape; on a phone held upright the picture is turned.
                    modifier = Modifier.fillMaxWidth().aspectRatio(size.height.toFloat() / size.width),
                )
                problem?.let { Text(it, color = Danger) }
                Text(tr("Or close this and type the code it shows."), color = Muted, style = MaterialTheme.typography.bodySmall)
                ActionButton(tr("Close"), onClose)
            }
        }
    }
}

/** The back camera feeding a preview and, a few times a second, the core's QR reader. */
private class QrCamera(
    ctx: Context,
) {
    private val manager = ctx.getSystemService(Context.CAMERA_SERVICE) as CameraManager
    private val id: String? =
        runCatching {
            manager.cameraIdList.firstOrNull {
                manager.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING) == CameraCharacteristics.LENS_FACING_BACK
            } ?: manager.cameraIdList.firstOrNull()
        }.getOrNull()

    /** The frame size: about 720p, plenty for a QR code on a TV across the room. */
    val size: Size =
        id
            ?.let { manager.getCameraCharacteristics(it).get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP) }
            ?.getOutputSizes(ImageFormat.YUV_420_888)
            ?.filter { it.width <= 1280 && it.height <= 1280 }
            ?.maxByOrNull { it.width * it.height }
            ?: Size(640, 480)

    private var thread: HandlerThread? = null
    private var device: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var reader: ImageReader? = null
    private val reading = AtomicBoolean(false)
    private val done = AtomicBoolean(false)

    @SuppressLint("MissingPermission")
    fun start(
        texture: SurfaceTexture,
        onCode: (String) -> Unit,
        onError: (String) -> Unit,
    ) {
        val cam = id ?: return onError(tr("No camera on this device."))
        val t = HandlerThread("qr").also { it.start() }
        thread = t
        val handler = Handler(t.looper)
        texture.setDefaultBufferSize(size.width, size.height)
        val preview = Surface(texture)
        val r = ImageReader.newInstance(size.width, size.height, ImageFormat.YUV_420_888, 2)
        reader = r
        r.setOnImageAvailableListener({ ir ->
            val image = ir.acquireLatestImage() ?: return@setOnImageAvailableListener
            // One frame at a time; the others are dropped while it is read.
            if (done.get() || !reading.compareAndSet(false, true)) {
                image.close()
                return@setOnImageAvailableListener
            }
            val plane = image.planes[0]
            val stride = plane.rowStride
            val (w, h) = image.width to image.height
            val buf = plane.buffer
            val luma = ByteArray(buf.remaining()).also { buf.get(it) }
            image.close()
            Thread {
                try {
                    be.alwyapps.watch.core
                        .qrSignInCode(luma, w.toUInt(), h.toUInt(), stride.toUInt())
                        ?.let { if (done.compareAndSet(false, true)) onCode(it) }
                } finally {
                    reading.set(false)
                }
            }.start()
        }, handler)
        try {
            manager.openCamera(
                cam,
                object : CameraDevice.StateCallback() {
                    override fun onOpened(d: CameraDevice) {
                        device = d
                        @Suppress("DEPRECATION")
                        d.createCaptureSession(
                            listOf(preview, r.surface),
                            object : CameraCaptureSession.StateCallback() {
                                override fun onConfigured(s: CameraCaptureSession) {
                                    session = s
                                    val req =
                                        d.createCaptureRequest(CameraDevice.TEMPLATE_PREVIEW).apply {
                                            addTarget(preview)
                                            addTarget(r.surface)
                                            set(CaptureRequest.CONTROL_AF_MODE, CaptureRequest.CONTROL_AF_MODE_CONTINUOUS_PICTURE)
                                        }
                                    runCatching { s.setRepeatingRequest(req.build(), null, handler) }
                                }

                                override fun onConfigureFailed(s: CameraCaptureSession) = onError(tr("The camera could not start."))
                            },
                            handler,
                        )
                    }

                    override fun onDisconnected(d: CameraDevice) = d.close()

                    override fun onError(
                        d: CameraDevice,
                        error: Int,
                    ) {
                        d.close()
                        onError(tr("The camera could not start."))
                    }
                },
                handler,
            )
        } catch (e: Exception) {
            onError(tr("The camera could not start."))
        }
    }

    fun stop() {
        done.set(true)
        runCatching { session?.close() }
        runCatching { device?.close() }
        runCatching { reader?.close() }
        thread?.quitSafely()
        session = null
        device = null
        reader = null
        thread = null
    }
}
