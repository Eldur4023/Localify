package dev.lux.desktop

import android.annotation.SuppressLint
import android.Manifest
import android.app.Activity
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.os.Build
import android.content.Intent
import android.os.Bundle
import android.net.Uri
import android.webkit.JavascriptInterface
import android.webkit.ValueCallback
import android.webkit.WebChromeClient
import android.webkit.WebView
import java.io.File

class MainActivity : Activity() {
    private lateinit var web: WebView
    // <input type=file> de la página (foto de perfil, portadas): selector de imágenes del sistema
    private var elegirArchivo: ValueCallback<Array<Uri>>? = null

    /**
     * Puente mínimo JS → Android: el shim de audio del WebView (web-audio.js)
     * avisa de cuándo hay que sostener el proceso con el servicio en primer
     * plano. Solo eso — el resto de la app vive en el servidor loopback.
     */
    class AndroidBridge(private val activity: MainActivity) {
        @JavascriptInterface
        fun startPlayback() {
            activity.runOnUiThread {
                val i = Intent(activity, PlaybackService::class.java)
                activity.startForegroundService(i)
            }
        }

        /** Estado para la MediaSession del sistema (pantalla de bloqueo, notificación, auriculares). */
        @JavascriptInterface
        fun setMedia(titulo: String, artista: String, duracionMs: Long, sonando: Boolean, posicionMs: Long, arteUrl: String) {
            PlaybackService.actualizar(titulo, artista, duracionMs, sonando, posicionMs, arteUrl)
        }

        // El servicio vive mientras viva la tarea (descargas y servidor incluidos),
        // no solo mientras suena algo: pausar ya no lo para.
        @JavascriptInterface
        fun stopPlayback() {}
    }

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        web = WebView(this).apply {
            settings.javaScriptEnabled = true
            settings.domStorageEnabled = true
            settings.mediaPlaybackRequiresUserGesture = false
            addJavascriptInterface(AndroidBridge(this@MainActivity), "AndroidBridge")
            webChromeClient = object : WebChromeClient() {
                override fun onShowFileChooser(w: WebView, cb: ValueCallback<Array<Uri>>, p: FileChooserParams): Boolean {
                    elegirArchivo?.onReceiveValue(null)
                    elegirArchivo = cb
                    startActivityForResult(Intent(Intent.ACTION_GET_CONTENT).addCategory(Intent.CATEGORY_OPENABLE).setType("image/*"), 7)
                    return true
                }
            }
        }
        // Depuración remota del WebView solo en builds debug (chrome://inspect).
        if (applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0) WebView.setWebContentsDebuggingEnabled(true)
        // Sin barra de título: la app dibuja la suya. Barras del sistema a juego con el fondo.
        window.statusBarColor = 0xFF121212.toInt()
        window.navigationBarColor = 0xFF121212.toInt()
        setContentView(web)
        // Android 13+: sin este permiso la notificación multimedia no se ve.
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED)
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        // Botones del sistema → la página (web-audio.js: window.__localifyNative)
        PlaybackService.alComando = { cmd, arg ->
            runOnUiThread { web.evaluateJavascript("window.__localifyNative&&window.__localifyNative.cmd('$cmd',$arg)", null) }
        }
        // Sin servicio en primer plano, con la pantalla apagada Android corta la
        // red a la app y las descargas (y la música) mueren.
        startForegroundService(Intent(this, PlaybackService::class.java))

        // Arranque en frío (~7 s: compilar la app y migrar): pantalla de carga
        // oscura en vez de un WebView en blanco que parece colgado.
        web.setBackgroundColor(0xFF121212.toInt())
        web.loadData(
            "<body style='margin:0;background:#121212;color:#1ed760;font:600 28px sans-serif;" +
                "display:flex;align-items:center;justify-content:center;height:100vh'>Localify</body>",
            "text/html", "utf-8"
        )
        // Compiling the app takes a moment: keep it off the UI thread.
        val dir = File(filesDir, "lux").absolutePath
        Thread {
            val port = LuxRuntime.ensureStarted(dir)
            // El servidor contesta 503 "starting" mientras corren las migraciones
            // (más lentas en una instalación limpia): cargar antes dejaría el
            // WebView clavado en ese error.
            if (port > 0) {
                val limite = System.currentTimeMillis() + 60_000
                while (System.currentTimeMillis() < limite) {
                    val ok = try {
                        (java.net.URL("http://127.0.0.1:$port/api/ready").openConnection() as java.net.HttpURLConnection)
                            .run { connectTimeout = 1000; readTimeout = 1000; responseCode == 200 }
                    } catch (e: Exception) { false }
                    if (ok) break
                    Thread.sleep(150)
                }
            }
            runOnUiThread {
                if (port > 0) web.loadUrl("http://127.0.0.1:$port/")
                else web.loadData("Lux failed to start; see logcat (tag luxdesktop).", "text/plain", "utf-8")
            }
        }.start()
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == 7) {
            elegirArchivo?.onReceiveValue(data?.data?.takeIf { resultCode == RESULT_OK }?.let { arrayOf(it) })
            elegirArchivo = null
        } else super.onActivityResult(requestCode, resultCode, data)
    }

    // Atrás cierra antes lo que esté superpuesto (cajón, vista de la canción, cola)
    override fun onBackPressed() {
        web.evaluateJavascript("window.__localifyAtras ? window.__localifyAtras() : false") { cerrado ->
            if (cerrado != "true") { if (web.canGoBack()) web.goBack() else super.onBackPressed() }
        }
    }

    override fun onDestroy() {
        super.onDestroy()
        PlaybackService.alComando = null
        if (isFinishing) {
            stopService(Intent(this, PlaybackService::class.java))
            // Sin apagado elegante: LuxRuntime.shutdown() espera al servidor y al
            // pool de la BD en el hilo principal (30 s o más) y la app reabierta
            // se quedaba colgada detrás. SQLite en WAL aguanta una terminación
            // brusca. Y Lux no puede reiniciarse dentro del mismo proceso (su
            // registro de BD sigue "activo" con el pool parado), así que la
            // próxima apertura tiene que ser un arranque en frío.
            android.os.Process.killProcess(android.os.Process.myPid())
        }
    }
}
