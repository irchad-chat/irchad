plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val rustOut = layout.buildDirectory.dir("rust")

android {
    namespace = "party.gentoo.irchad"
    compileSdk = 36

    defaultConfig {
        applicationId = "party.gentoo.irchad"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.1"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // shortcut: signed with the debug key until CI has a release key (D40).
            signingConfig = signingConfigs.getByName("debug")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    sourceSets["main"].apply {
        java.srcDir(rustOut.map { it.dir("kotlin") })
        jniLibs.srcDir(rustOut.map { it.dir("jniLibs") })
    }
}

kotlin {
    compilerOptions { jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17) }
}

// libchad_ffi.so and its Kotlin bindings (UniFFI), from the Cargo workspace.
val buildRust by tasks.registering(Exec::class) {
    workingDir = rootDir
    commandLine("./build-rust.sh")
    environment("ANDROID_HOME", android.sdkDirectory.path)
}
tasks.named("preBuild") { dependsOn(buildRust) }

dependencies {
    val bom = platform("androidx.compose:compose-bom:2025.09.00")
    implementation(bom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-core")
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("io.livekit:livekit-android:2.29.2")
    debugImplementation("androidx.compose.ui:ui-tooling")
}
