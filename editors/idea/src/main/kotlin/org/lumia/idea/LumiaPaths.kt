package org.lumia.idea

import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import java.io.File

/** Resolves the `lumia` binary and a PATH that includes cargo/local bin. */
object LumiaPaths {
    private val home: String get() = System.getProperty("user.home")
    private val pathSep: String get() = File.pathSeparator

    fun resolveLumia(): String {
        val configured = LumiaSettings.getInstance().lspPath.trim()
        val candidates = listOf(
            configured,
            "$home/.cargo/bin/lumia",
            "$home/.cargo/bin/lumia.exe",
            "$home/.local/bin/lumia",
            "$home/.local/bin/lumia.exe",
        )
        return candidates.firstOrNull { it.isNotBlank() && File(it).canExecute() }
            ?: configured.ifBlank { "lumia" }
    }

    fun cargoBinDir(): String = "$home/.cargo/bin"

    fun pathWithExtras(existing: String? = System.getenv("PATH")): String {
        val extras = listOf(cargoBinDir(), "$home/.local/bin")
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

    fun isLumiaFile(file: VirtualFile): Boolean =
        file.extension.equals("lm", ignoreCase = true) ||
            file.fileType === LumiaFileType.INSTANCE
}
