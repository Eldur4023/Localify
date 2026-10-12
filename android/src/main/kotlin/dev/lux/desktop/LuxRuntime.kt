package dev.lux.desktop

/** Bridge to libluxapp.so (src/android.cpp): runs the embedded Lux app's HTTP server. */
object LuxRuntime {
    init { System.loadLibrary("luxapp") }

    @Volatile private var port = -1

    /** Starts the server once per process; returns its loopback port, or -1 if the app failed to compile. */
    @Synchronized fun ensureStarted(dataDir: String): Int {
        if (port < 0) port = start(dataDir)
        return port
    }

    @Synchronized fun shutdown() { stop(); port = -1 }

    @JvmStatic private external fun start(dataDir: String): Int
    @JvmStatic private external fun stop()
}
