import java.util.Properties

plugins {
    id("com.android.application") version "8.5.2"
    id("org.jetbrains.kotlin.android") version "2.0.0"
}

// Firma de release: keystore y claves viven fuera del repo (~/android-build).
val firma = Properties().apply {
    val f = file(System.getProperty("user.home") + "/android-build/signing.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

android {
    namespace = "dev.lux.desktop"
    compileSdk = 34
    ndkVersion = "27.0.12077973"

    defaultConfig {
        applicationId = "es.localify.app"
        minSdk = 28                          // posix_spawn / getrandom need API 28
        targetSdk = 34
        versionCode = 1
        versionName = "1.0"
        ndk { abiFilters += "arm64-v8a" }
        externalNativeBuild {
            cmake {
                // Solo la librería: los ejecutables-tool de lux (lux-bin,
                // crypto_check...) no van en la APK y no enlazan en el NDK.
                targets += "luxapp"
                // Dependencias cruzadas (arm64) en ~/android-deps: sqlite3 y
                // libcurl+openssl (ver android/README.md, "Dependencies").
                val deps = System.getProperty("user.home") + "/android-deps/arm64"
                arguments += listOf(
                    "-DANDROID_STL=c++_shared",
                    "-DCMAKE_BUILD_TYPE=Release",
                    "-DLUX_EXAMPLES=OFF",
                    "-DSQLITE3_INC=$deps/include",
                    "-DSQLITE3_LIB=$deps/lib/libsqlite3.a",
                    // pkg-config confinado a las deps arm64: el del host metería
                    // -I/usr/include en la compilación cruzada y revienta
                    // el traductor ARM del emulador se atraganta con los IFUNC
                    // de las outline atomics: sin ellas, dentro y fuera
                    "-DCMAKE_C_FLAGS=-mno-outline-atomics",
                    "-DCMAKE_CXX_FLAGS=-mno-outline-atomics",
                    "-DPKG_CONFIG_EXECUTABLE=$deps/bin/pkg-config-arm64",
                    "-DCMAKE_SHARED_LINKER_FLAGS=-L$deps/lib -lcurl -lssl -lcrypto -llog -Wl,-z,max-page-size=16384"
                )
            }
        }
    }
    externalNativeBuild { cmake { path = file("../CMakeLists.txt") } }
    signingConfigs {
        if (firma.containsKey("storeFile")) create("release") {
            storeFile = file(firma.getProperty("storeFile"))
            storePassword = firma.getProperty("storePassword")
            keyAlias = firma.getProperty("keyAlias")
            keyPassword = firma.getProperty("keyPassword")
        }
    }
    buildTypes {
        release {
            signingConfig = signingConfigs.findByName("release")
            isMinifyEnabled = false   // el código es una Activity y un servicio; lo pesado es nativo
        }
    }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
}
