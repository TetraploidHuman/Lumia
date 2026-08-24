package org.lumi.idea

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import java.io.File

/** Resolves the `lumi` binary and a PATH that includes cargo/local bin. */
object LumiPaths {
    private val home: String get() = System.getProperty("user.home")
    private val pathSep: String get() = File.pathSeparator
    private val cacheLock = Any()
    @Volatile private var cachedLibraryPath: String? = null

    /**
     * Locate the `lumi` CLI. Prefers an explicit settings path, then project/repo
     * `target/release/lumi`, then common install locations. Never returns legacy `lumia`.
     */
    fun resolveLumi(project: Project? = null): String = resolveLumiBinary(project)

    private fun resolveLumiBinary(project: Project? = null): String {
        val configured = LumiSettings.getInstance().lspPath.trim()
        if (configured.isNotBlank() && configured.contains(File.separator)) {
            val file = File(configured)
            if (file.canExecute() && !isLegacyLumiaBinary(configured)) {
                return configured
            }
        }

        val candidates = mutableListOf<String>()
        System.getenv("LUMI")?.takeIf { it.isNotBlank() }?.let { candidates += it }
        System.getenv("LUMI_HOME")?.let { root ->
            candidates += "$root/target/release/lumi"
            candidates += "$root/target/debug/lumi"
            candidates += "$root/bin/lumi"
        }
        project?.basePath?.let { candidates += discoverFromProjectRoot(it) }
        candidates += listOf(
            "$home/Lumia/target/release/lumi",
            "$home/Lumia/target/debug/lumi",
            "$home/lumi/target/release/lumi",
            "$home/.cargo/bin/lumi",
            "$home/.local/bin/lumi",
        )
        if (configured.isNotBlank() && configured != "lumia") {
            candidates += configured
        }

        return candidates
            .distinct()
            .firstOrNull { path -> path.isNotBlank() && File(path).canExecute() && !isLegacyLumiaBinary(path) }
            ?: configured.ifBlank { "lumi" }
    }

    fun findEnvSh(project: Project? = null, lumiBinary: String? = null): File? {
        project?.basePath?.let { discoverEnvShFrom(File(it)) }?.let { return it }
        File("$home/Lumia/scripts/env.sh").takeIf { it.isFile }?.let { return it }
        lumiBinary?.let {
            val file = File(it)
            if (file.name == "lumi-run.sh") {
                file.parentFile?.resolve("env.sh")?.takeIf { f -> f.isFile }?.let { f -> return f }
            }
            file.parentFile?.parentFile?.parentFile?.resolve("scripts/env.sh")
                ?.takeIf { f -> f.isFile }
                ?.let { f -> return f }
        }
        return null
    }

    private fun discoverEnvShFrom(start: File): File? {
        var dir = start
        repeat(8) {
            val candidate = File(dir, "scripts/env.sh")
            if (candidate.isFile) return candidate
            dir = dir.parentFile ?: return null
        }
        return null
    }

    /** NixOS: `lumi` links against store libs; IDEA must inherit `env.sh` paths. */
    fun libraryPathWithExtras(project: Project? = null, lumiBinary: String? = null): String {
        val existing = System.getenv("LD_LIBRARY_PATH").orEmpty()
        cachedLibraryPath?.let { return mergePaths(it, existing) }

        val envSh = findEnvSh(project, lumiBinary) ?: return existing
        val resolved = runCatching {
            val proc = ProcessBuilder(
                "bash",
                "-c",
                "source \"${envSh.path}\" >/dev/null 2>&1; printf '%s' \"\${LD_LIBRARY_PATH:-}\"",
            ).redirectErrorStream(true).start()
            val out = proc.inputStream.bufferedReader().readText().trim()
            proc.waitFor()
            out
        }.getOrDefault("")
        if (resolved.isNotBlank()) {
            synchronized(cacheLock) { cachedLibraryPath = resolved }
        }
        return mergePaths(resolved, existing)
    }

    fun applyRuntimeEnvironment(
        commandLine: GeneralCommandLine,
        project: Project? = null,
        lumiBinary: String? = null,
    ): GeneralCommandLine {
        val libPath = libraryPathWithExtras(project, lumiBinary)
        commandLine.withEnvironment("PATH", pathWithExtras())
        if (libPath.isNotBlank()) {
            commandLine.withEnvironment("LD_LIBRARY_PATH", libPath)
            commandLine.withEnvironment("LIBRARY_PATH", libPath)
        }
        return commandLine
    }

    private fun mergePaths(vararg paths: String): String =
        paths.flatMap { it.split(pathSep[0]) }.filter { it.isNotBlank() }.distinct().joinToString(pathSep)

    private fun isLegacyLumiaBinary(path: String): Boolean {
        val name = File(path).name
        return name == "lumia" || name == "lumia.exe"
    }

    private fun discoverFromProjectRoot(basePath: String): List<String> {
        val found = mutableListOf<String>()
        var dir = File(basePath)
        repeat(6) {
            found += File(dir, "target/release/lumi").path
            found += File(dir, "target/debug/lumi").path
            val parent = dir.parentFile ?: return@repeat
            dir = parent
        }
        File(basePath).parentFile?.let { parent ->
            for (name in listOf("Lumia", "Lumi", "lumi")) {
                found += File(parent, "$name/target/release/lumi").path
                found += File(parent, "$name/target/debug/lumi").path
            }
        }
        return found
    }

    fun cargoBinDir(): String = "$home/.cargo/bin"

    fun pathWithExtras(existing: String? = System.getenv("PATH")): String {
        val extras = listOf(cargoBinDir(), "$home/.local/bin", "$home/Lumia/target/release")
        val parts = (extras + listOfNotNull(existing?.takeIf { it.isNotBlank() }))
            .flatMap { it.split(pathSep[0]) }
            .filter { it.isNotBlank() }
            .distinct()
        return parts.joinToString(pathSep)
    }

    /**
     * Prefer `src/main.lm` / `main.lm`, else the focused `.lm` file.
     */
    fun resolveProjectEntry(project: Project, contextFile: VirtualFile? = null): String? {
        val base = project.basePath
        if (base != null) {
            for (rel in listOf("src/main.lm", "main.lm", "examples/hello.lm")) {
                val p = "$base/$rel"
                if (File(p).isFile) return p
            }
        }
        val ctx = contextFile ?: return null
        if (ctx.extension.equals("lm", ignoreCase = true)) return ctx.path
        return null
    }

    fun isLumiFile(file: VirtualFile): Boolean =
        file.extension.equals("lm", ignoreCase = true) ||
            file.fileType === LumiFileType.INSTANCE
}
