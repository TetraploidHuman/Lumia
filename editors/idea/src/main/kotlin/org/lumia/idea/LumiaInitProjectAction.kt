package org.lumia.idea

import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.Messages
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.openapi.vfs.VirtualFile
import java.io.File

/**
 * Initialize an existing IDE project with Lumia.toml + src/main.lm (skips files that already exist).
 */
class LumiaInitProjectAction :
    AnAction(
        "Lumia Project",
        "Create Lumia.toml and src/main.lm in the project root",
        LumiaIcons.FILE,
    ),
    DumbAware {

    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        val baseDir = projectBaseDir(project) ?: return
        val defaultName = File(baseDir.path).name
        val input =
            Messages.showInputDialog(
                project,
                "Package name for Lumia.toml:",
                "New Lumia Project",
                LumiaIcons.FILE,
                defaultName,
                null,
            ) ?: return
        val err = LumiaProjectScaffold.validatePackageName(input)
        if (err != null) {
            Messages.showErrorDialog(project, err, "Lumia")
            return
        }
        try {
            val created = LumiaProjectScaffold.initMissing(baseDir, input.trim())
            if (created.isEmpty()) {
                Messages.showInfoMessage(
                    project,
                    "Lumia.toml and src/main.lm already exist.",
                    "Lumia",
                )
                return
            }
            val main = baseDir.findFileByRelativePath("src/main.lm")
            if (main != null) {
                FileEditorManager.getInstance(project).openFile(main, true)
            }
            LumiaProjectActivity.kick(project, "initProject")
            Messages.showInfoMessage(
                project,
                "Created: ${created.joinToString(", ")}",
                "Lumia",
            )
        } catch (ex: Exception) {
            Messages.showErrorDialog(project, ex.message ?: "Failed to create Lumia project files", "Lumia")
        }
    }

    private fun projectBaseDir(project: Project): VirtualFile? {
        val basePath = project.basePath
        if (basePath == null) {
            Messages.showErrorDialog(project, "No project directory is open.", "Lumia")
            return null
        }
        return LocalFileSystem.getInstance().findFileByPath(basePath)
            ?: run {
                Messages.showErrorDialog(project, "Cannot access project directory.", "Lumia")
                null
            }
    }
}
