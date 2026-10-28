// Code here must not import android.*, androidx.*, org.json.* or com.facebook.*:
// time and platform constants come in as parameters.
plugins {
    kotlin("jvm") version "2.2.0"
}

group = "ai.gxlself.remotescreen"
version = "0.1.0"

kotlin {
    jvmToolchain(17)
}

dependencies {
    testImplementation("junit:junit:4.13.2")
}

tasks.test {
    useJUnit()
    testLogging {
        events("failed")
        exceptionFormat = org.gradle.api.tasks.testing.logging.TestExceptionFormat.FULL
    }
}
