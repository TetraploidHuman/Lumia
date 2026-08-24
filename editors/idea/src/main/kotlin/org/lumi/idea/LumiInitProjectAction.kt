package org.lumi.idea

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
 * Initialize an existing IDE project with Lumi.toml + src/main.lm (skips files that already exist).
 */
class LumiInitProjectAction :
    AnAction(
        "Lumi Project",
        "Create Lumi.toml and src/main.lm in the project root",
        LumiIcons.FILE,
    ),
    DumbAware {

    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        val baseDir = projectBaseDir(project) ?: return
        val defaultName = File(baseDir.path).name
        val input =
            Messages.showInputDialog(
                project,
                "Package name for Lumi.toml:",
                "New Lumi Project",
                LumiIcons.FILE,
                defaultName,
                null,
            ) ?: return
        val err = LumiProjectScaffold.validatePackageName(input)
        if (err != null) {
            Messages.showErrorDialog(project, err, "Lumi")
            return
        }
        try {
            val created = LumiProjectScaffold.initMissing(baseDir, input.trim())
            if (created.isEmpty()) {
                Messages.showInfoMessage(
                    project,
                    "Lumi.toml and src/main.lm already exist.",
                    "Lumi",
                )
                return
            }
            val main = baseDir.findFileByRelativePath("src/main.lm")
            if (main != null) {
                FileEditorManager.getInstance(project).openFile(main, true)
            }
            LumiProjectActivity.kick(project, "initProject")
            Messages.showInfoMessage(
                project,
                "Created: ${created.joinToString(", ")}",
                "Lumi",
            )
        } catch (ex: Exception) {
            Messages.showErrorDialog(project, ex.message ?: "Failed to create Lumi project files", "Lumi")
        }
    }

    private fun projectBaseDir(project: Project): VirtualFile? {
        val basePath = project.basePath
        if (basePath == null) {
            Messages.showErrorDialog(project, "No project directory is open.", "Lumi")
            return null
        }
        return LocalFileSystem.getInstance().findFileByPath(basePath)
            ?: run {
                Messages.showErrorDialog(project, "Cannot access project directory.", "Lumi")
                null
            }
    }
}
