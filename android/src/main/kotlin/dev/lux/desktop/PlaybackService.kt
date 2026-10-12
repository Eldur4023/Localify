package dev.lux.desktop

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.AudioManager
import android.media.MediaMetadata
import android.media.session.MediaSession
import android.media.session.PlaybackState
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import java.net.URL

/**
 * Servicio en primer plano con la MediaSession del sistema.
 *
 * Sostiene el proceso (el servidor Lux, las descargas y el WebView con su
 * <audio>) con la pantalla apagada y da a Android lo que el WebView no publica
 * por su cuenta: título/portada/posición en la pantalla de bloqueo y los
 * botones de la notificación, los auriculares y el coche. Los botones vuelven
 * a la página por `alComando` (ver MainActivity y web-audio.js).
 */
class PlaybackService : Service() {
    private lateinit var sesion: MediaSession
    private val ui = Handler(Looper.getMainLooper())

    private var titulo = "Localify"
    private var artista = ""
    private var duracionMs = 0L
    private var posicionMs = 0L
    private var sonando = false
    private var arte: Bitmap? = null
    private var arteUrl = ""

    // Auriculares desenchufados / Bluetooth caído: pausar, no seguir por el altavoz.
    private val ruidoso = object : BroadcastReceiver() {
        override fun onReceive(c: Context?, i: Intent?) { alComando?.invoke("pause", 0) }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        instancia = this
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CANAL, "Reproducción", NotificationManager.IMPORTANCE_LOW)
        )
        sesion = MediaSession(this, "Localify").apply {
            setCallback(object : MediaSession.Callback() {
                override fun onPlay() { alComando?.invoke("play", 0) }
                override fun onPause() { alComando?.invoke("pause", 0) }
                override fun onSkipToNext() { alComando?.invoke("next", 0) }
                override fun onSkipToPrevious() { alComando?.invoke("previous", 0) }
                override fun onSeekTo(pos: Long) { alComando?.invoke("seek", pos) }
            })
            isActive = true
        }
        registerReceiver(ruidoso, IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY))
        refrescar()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_STOP -> { stopForeground(STOP_FOREGROUND_REMOVE); stopSelf(); return START_NOT_STICKY }
            ACCION_PLAY -> alComando?.invoke("play", 0)
            ACCION_PAUSE -> alComando?.invoke("pause", 0)
            ACCION_NEXT -> alComando?.invoke("next", 0)
            ACCION_PREV -> alComando?.invoke("previous", 0)
        }
        refrescar()
        return START_STICKY
    }

    // Quitar la app de recientes cierra el servicio (y deja morir el proceso).
    override fun onTaskRemoved(rootIntent: Intent?) {
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
        // Sin la tarea no queda nada que sirva: proceso fuera (ver MainActivity.onDestroy).
        android.os.Process.killProcess(android.os.Process.myPid())
    }

    override fun onDestroy() {
        instancia = null
        unregisterReceiver(ruidoso)
        sesion.release()
        super.onDestroy()
    }

    /** Lo llama el puente JS (hilo del WebView): copia el estado y repinta. */
    private fun actualizar(t: String, a: String, dur: Long, suena: Boolean, pos: Long, url: String) {
        titulo = t.ifBlank { "Localify" }; artista = a; duracionMs = dur
        sonando = suena; posicionMs = pos
        if (url != arteUrl) {
            arteUrl = url; arte = null
            if (url.isNotEmpty()) Thread {
                val b = try { URL(url).openStream().use { BitmapFactory.decodeStream(it) } } catch (e: Exception) { null }
                ui.post { if (url == arteUrl) { arte = b; refrescar() } }
            }.start()
        }
        ui.post { refrescar() }
    }

    private fun accion(icono: Int, etiqueta: String, accion: String): Notification.Action =
        Notification.Action.Builder(
            android.graphics.drawable.Icon.createWithResource(this, icono), etiqueta,
            PendingIntent.getService(this, accion.hashCode(), Intent(this, PlaybackService::class.java).setAction(accion),
                PendingIntent.FLAG_IMMUTABLE)
        ).build()

    private fun refrescar() {
        sesion.setMetadata(
            MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, titulo)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, artista)
                .putLong(MediaMetadata.METADATA_KEY_DURATION, duracionMs)
                .putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, arte)
                .build()
        )
        sesion.setPlaybackState(
            PlaybackState.Builder()
                .setActions(
                    PlaybackState.ACTION_PLAY or PlaybackState.ACTION_PAUSE or PlaybackState.ACTION_PLAY_PAUSE or
                        PlaybackState.ACTION_SKIP_TO_NEXT or PlaybackState.ACTION_SKIP_TO_PREVIOUS or PlaybackState.ACTION_SEEK_TO
                )
                .setState(if (sonando) PlaybackState.STATE_PLAYING else PlaybackState.STATE_PAUSED, posicionMs, 1f)
                .build()
        )
        val abrir = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE
        )
        val n = Notification.Builder(this, CANAL)
            .setSmallIcon(android.R.drawable.ic_media_play)
            .setContentTitle(titulo)
            .setContentText(artista.ifBlank { "Localify" })
            .setLargeIcon(arte)
            .setContentIntent(abrir)
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOngoing(true)
            .addAction(accion(android.R.drawable.ic_media_previous, "Anterior", ACCION_PREV))
            .addAction(
                if (sonando) accion(android.R.drawable.ic_media_pause, "Pausa", ACCION_PAUSE)
                else accion(android.R.drawable.ic_media_play, "Reproducir", ACCION_PLAY)
            )
            .addAction(accion(android.R.drawable.ic_media_next, "Siguiente", ACCION_NEXT))
            .setStyle(Notification.MediaStyle().setMediaSession(sesion.sessionToken).setShowActionsInCompactView(0, 1, 2))
            .build()
        startForeground(ID_NOTIFICACION, n)
    }

    companion object {
        const val ACTION_STOP = "dev.lux.desktop.STOP"
        const val ACCION_PLAY = "dev.lux.desktop.PLAY"
        const val ACCION_PAUSE = "dev.lux.desktop.PAUSE"
        const val ACCION_NEXT = "dev.lux.desktop.NEXT"
        const val ACCION_PREV = "dev.lux.desktop.PREV"
        const val CANAL = "playback"
        const val ID_NOTIFICACION = 1

        /** Lo fija MainActivity: manda el comando ("play", "next", "seek"...) a la página. */
        @Volatile var alComando: ((String, Long) -> Unit)? = null
        @Volatile private var instancia: PlaybackService? = null

        fun actualizar(t: String, a: String, dur: Long, suena: Boolean, pos: Long, url: String) {
            instancia?.actualizar(t, a, dur, suena, pos, url)
        }
    }
}
