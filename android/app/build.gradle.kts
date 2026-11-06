plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

val rustOut = layout.buildDirectory.dir("rust")

// The workspace version (Cargo.toml), so the APK matches the release tag.
val cargoVersion = rootDir.resolve("../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }.substringAfter('"').substringBefore('"')
// Android only installs an update with a higher versionCode: 1.2.3 -> 1002003.
val cargoVersionCode = cargoVersion.substringBefore('-').split('.').map { it.toInt() }
    .let { (major, minor, patch) -> major * 1_000_000 + minor * 1_000 + patch }
// release.yml decodes the keystore here and sets the password (D40).
val releaseKeystore = System.getenv("IRCHAD_KEYSTORE")?.let { file(it) }

android {
    namespace = "party.gentoo.irchad"
    compileSdk = 36

    defaultConfig {
        applicationId = "party.gentoo.irchad"
        minSdk = 29
        targetSdk = 36
        versionCode = cargoVersionCode
        versionName = cargoVersion
        // Only the ABIs build-rust.sh built (the same ABIS), which also drops
        // LiveKit's 32-bit libraries.
        ndk { abiFilters += (System.getenv("ABIS") ?: "x86_64").split(" ").filter { it.isNotBlank() } }
    }

    // full: sideloaded/Obtainium APK. play: Play Store bundle, without the
    // specialUse "Stay connected" service Play reviews strictly (src/play).
    // play is a separate app (its own ID and upload key), so both can be installed.
    flavorDimensions += "store"
    productFlavors {
        create("full") {
            dimension = "store"
            buildConfigField("boolean", "STAY_CONNECTED", "true")
        }
        create("play") {
            dimension = "store"
            applicationId = "chat.irchad.IrChad"
            buildConfigField("boolean", "STAY_CONNECTED", "false")
        }
    }

    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                storeFile = releaseKeystore
                storePassword = System.getenv("IRCHAD_KEYSTORE_PASSWORD")
                keyAlias = "irchad"
                keyPassword = System.getenv("IRCHAD_KEYSTORE_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Local release builds without the keystore get the debug key.
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
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
    // Push without FCM (D39); it also decrypts the pushes (RFC 8291).
    implementation("org.unifiedpush.android:connector:3.3.5") {
        // Plain Tink brings full protobuf, which clashes with LiveKit's protobuf-lite.
        exclude(group = "com.google.crypto.tink", module = "tink")
    }
    implementation("com.google.crypto.tink:tink-android:1.23.0")
    debugImplementation("androidx.compose.ui:ui-tooling")
    testImplementation("junit:junit:4.13.2")
}
