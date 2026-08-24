package org.lumi.idea

import com.intellij.openapi.application.WriteAction
import com.intellij.openapi.vfs.VfsUtil
import com.intellij.openapi.vfs.VirtualFile
import java.io.File

/** Creates `Lumi.toml` + `src/main.lm` (aligned with `lumi pkg init` + hello template). */
object LumiProjectScaffold {
    private val PACKAGE_RE = Regex("^[A-Za-z0-9_-]+$")

    fun validatePackageName(name: String): String? {
        val n = name.trim()
        return when {
            n.isEmpty() -> "Package name is required"
            !PACKAGE_RE.matches(n) -> "Package name must be [A-Za-z0-9_-]+"
            else -> null
        }
    }

    fun moduleNameFromPackage(packageName: String): String = packageName.replace('-', '_')

    fun renderToml(packageName: String): String =
        """
        [package]
        name = "$packageName"
        version = "0.1.0"
        """.trimIndent() + "\n"

    fun renderMainLm(moduleName: String): String =
        """
        module $moduleName

        import lumi.io.{println}

        val main = {
            println(42)
        }
        """.trimIndent() + "\n"

    fun hasManifest(baseDir: VirtualFile): Boolean = baseDir.findChild("Lumi.toml") != null

    /** Create scaffold on disk (before a VFS project root exists). */
    fun createOnDisk(projectDir: String, packageName: String) {
        val pkg = packageName.trim()
        validatePackageName(pkg)?.let { throw IllegalArgumentException(it) }
        val dir = File(projectDir)
        if (dir.exists()) {
            throw IllegalStateException("Directory already exists: ${dir.path}")
        }
        if (!dir.mkdirs()) {
            throw IllegalStateException("Cannot create directory: ${dir.path}")
        }
        val moduleName = moduleNameFromPackage(pkg)
        File(dir, "Lumi.toml").writeText(renderToml(pkg))
        val src = File(dir, "src")
        if (!src.mkdir()) {
            throw IllegalStateException("Cannot create directory: ${src.path}")
        }
        File(src, "main.lm").writeText(renderMainLm(moduleName))
    }

    /**
     * Write package manifest and entry `src/main.lm`. Returns the main source file.
     * @throws IllegalStateException if `Lumi.toml` already exists
     */
    fun create(baseDir: VirtualFile, packageName: String): VirtualFile {
        val pkg = packageName.trim()
        validatePackageName(pkg)?.let { throw IllegalArgumentException(it) }
        val moduleName = moduleNameFromPackage(pkg)
        return WriteAction.compute<VirtualFile, RuntimeException> {
            if (baseDir.findChild("Lumi.toml") != null) {
                throw IllegalStateException("Lumi.toml already exists in ${baseDir.path}")
            }
            val toml = baseDir.createChildData(this, "Lumi.toml")
            VfsUtil.saveText(toml, renderToml(pkg))
            val src = baseDir.findChild("src") ?: baseDir.createChildDirectory(this, "src")
            val mainLm = src.createChildData(this, "main.lm")
            VfsUtil.saveText(mainLm, renderMainLm(moduleName))
            mainLm
        }
    }

    /** Add missing scaffold files without overwriting existing ones. */
    fun initMissing(baseDir: VirtualFile, packageName: String): List<String> {
        val pkg = packageName.trim()
        validatePackageName(pkg)?.let { throw IllegalArgumentException(it) }
        val moduleName = moduleNameFromPackage(pkg)
        return WriteAction.compute<List<String>, RuntimeException> {
            val created = mutableListOf<String>()
            if (baseDir.findChild("Lumi.toml") == null) {
                val toml = baseDir.createChildData(this, "Lumi.toml")
                VfsUtil.saveText(toml, renderToml(pkg))
                created += "Lumi.toml"
            }
            val src = baseDir.findChild("src") ?: run {
                created += "src/"
                baseDir.createChildDirectory(this, "src")
            }
            if (src.findChild("main.lm") == null) {
                val mainLm = src.createChildData(this, "main.lm")
                VfsUtil.saveText(mainLm, renderMainLm(moduleName))
                created += "src/main.lm"
            }
            created
        }
    }
}
