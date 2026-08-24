plugins {
    id("java")
    id("org.jetbrains.kotlin.jvm") version "2.3.20"
    id("org.jetbrains.intellij.platform") version "2.6.0"
}

group = "org.lumi"
version = "0.3.10"

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
        id = "org.lumi.idea"
        name = "Lumi"
        version = project.version.toString()
        description = """
            Lumi language support powered by <code>lumi lsp</code>:
            diagnostics, completion, hover, go-to-definition, formatting, and outline.
            <br/>Build the CLI first: <code>source scripts/env.sh &amp;&amp; cargo build -p lumi --release</code>
        """.trimIndent()

        ideaVersion {
            sinceBuild = "262"
            untilBuild = "262.*"
        }

        vendor {
            name = "Lumi"
        }
    }
}

tasks {
    wrapper {
        gradleVersion = "8.13"
    }
}
