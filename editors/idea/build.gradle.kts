plugins {
    id("java")
    id("org.jetbrains.kotlin.jvm") version "2.3.20"
    id("org.jetbrains.intellij.platform") version "2.6.0"
}

group = "org.lumia"
version = "0.3.4"

repositories {
    mavenCentral()
    intellijPlatform {
        defaultRepositories()
    }
}

dependencies {
    intellijPlatform {
        // Matches the JetBrains native LSP API used by the sibling Lumi plugin.
        create("IU", "2026.2")
    }
}

kotlin {
    jvmToolchain(21)
}

intellijPlatform {
    buildSearchableOptions = false
    pluginConfiguration {
        id = "org.lumia.idea"
        name = "Lumia"
        version = project.version.toString()
        description = """
            Lumia language support powered by <code>lumia lsp</code>:
            diagnostics, completion, hover, go-to-definition, formatting, and outline.
            <br/>Build the CLI first: <code>source scripts/env.sh &amp;&amp; cargo build -p lumia --release</code>
        """.trimIndent()

        ideaVersion {
            sinceBuild = "262"
            untilBuild = "262.*"
        }

        vendor {
            name = "Lumia"
        }
    }
}

tasks {
    wrapper {
        gradleVersion = "8.13"
    }
}
