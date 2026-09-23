plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

dependencyLocking {
    lockAllConfigurations()
    lockMode.set(LockMode.STRICT)
}

val developmentChannel = providers.gradleProperty("zorkChannel").getOrElse("test")
require(developmentChannel in listOf("dev", "test")) { "zorkChannel must be dev or test" }
val profileBuild = providers.gradleProperty("zorkProfile").map(String::toBoolean).getOrElse(false)
val productVersion = (groovy.json.JsonSlurper().parse(rootProject.file("../../package.json")) as Map<*, *>)["version"] as String

android {
    namespace = "ing.zork.android"
    compileSdk = 36
    ndkVersion = "28.2.13676358"
    sourceSets.getByName("main").kotlin.srcDir(rootProject.file("third-party/rustls-platform-verifier/src"))

    defaultConfig {
        applicationId = "ing.zork.android"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = productVersion
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        manifestPlaceholders["zorkProfileable"] = profileBuild.toString()
        ndk { abiFilters += "arm64-v8a" }
    }
    buildFeatures { compose = true; resValues = true }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    sourceSets.getByName("main").jniLibs.srcDir(file("build/generated/jniLibs"))
    signingConfigs.getByName("debug") {
        providers.environmentVariable("ZORK_ANDROID_DEBUG_KEYSTORE").orNull?.let {
            storeFile = file(it)
        }
    }
    buildTypes {
        debug {
            applicationIdSuffix = ".$developmentChannel"
            resValue("string", "app_name", if (developmentChannel == "test") "Zork Test" else "Zork Dev")
            if (profileBuild) {
                isDebuggable = false
            }
        }
        release { isMinifyEnabled = false }
    }
    packaging { jniLibs.useLegacyPackaging = false }
}

dependencies {
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    implementation(platform("androidx.compose:compose-bom:2026.01.00"))
    implementation("androidx.activity:activity-compose:1.12.3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.10.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.10.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.2")
    implementation("io.noties.markwon:core:4.6.2")
    implementation("io.noties.markwon:ext-tables:4.6.2")
    implementation("io.noties.markwon:ext-strikethrough:4.6.2")

    debugImplementation("androidx.compose.ui:ui-tooling")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:rules:1.7.0")
}
