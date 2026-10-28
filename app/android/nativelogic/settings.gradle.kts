// Pure Kotlin/JVM logic shared by the Android native modules. It is a
// standalone build so its unit tests run without the Android SDK:
//   cd app/android && ./gradlew -p nativelogic test
pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
    }
}

rootProject.name = "nativelogic"
