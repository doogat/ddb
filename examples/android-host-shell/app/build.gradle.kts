plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.doogat.hostshell.app"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.doogat.hostshell"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "1.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

dependencies {
    implementation(project(":core-ddb"))
    implementation(project(":feature-bookmarks"))
    implementation(project(":feature-contacts"))

    implementation("androidx.activity:activity-compose:1.9.0")
    implementation("androidx.compose.material3:material3:1.3.0")
    implementation("androidx.compose.material:material-icons-extended:1.7.0")
    implementation("net.java.dev.jna:jna:5.16.0@aar")
}
