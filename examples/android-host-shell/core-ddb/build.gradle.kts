plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.doogat.hostshell.core"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    // UniFFI-generated Kotlin bindings expect JNA
    api("net.java.dev.jna:jna:5.16.0@aar")
    // The generated ddb_core.kt and libddb_core.so go in src/main/java and jniLibs/
}
