package org.lumi.idea

import com.intellij.ide.impl.ProjectUtil
import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.DialogWrapper
import com.intellij.openapi.ui.TextFieldWithBrowseButton
import com.intellij.openapi.ui.ValidationInfo
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.util.ui.FormBuilder
import java.io.File
import java.nio.file.Path
import javax.swing.JComponent
import javax.swing.JTextField

/**
 * File → New / Welcome screen: pick a directory, write scaffold files, open the project.
 */
class LumiCreateProjectAction :
    com.intellij.openapi.actionSystem.AnAction(
        "New Lumi Project…",
        "Create a Lumi package (Lumi.toml + src/main.lm) and open it",
        LumiIcons.FILE,
    ),
    DumbAware {

    override fun actionPerformed(e: com.intellij.openapi.actionSystem.AnActionEvent) {
        val dialog = LumiCreateProjectDialog()
        if (!dialog.showAndGet()) return
        val projectPath = dialog.projectPath
        val packageName = dialog.packageName
        ApplicationManager.getApplication().executeOnPooledThread {
            try {
                LumiProjectScaffold.createOnDisk(projectPath, packageName)
                ApplicationManager.getApplication().invokeLater {
                    val opened =
                        ProjectUtil.openOrImport(Path.of(projectPath), e.project, true)
                    if (opened != null) {
                        openMainIfPresent(opened)
                        LumiProjectActivity.kick(opened, "createProject")
                    }
                }
            } catch (ex: Exception) {
                ApplicationManager.getApplication().invokeLater {
                    com.intellij.openapi.ui.Messages.showErrorDialog(
                        e.project,
                        ex.message ?: "Failed to create Lumi project",
                        "Lumi",
                    )
                }
            }
        }
    }

    private fun openMainIfPresent(project: Project) {
        val base = project.basePath ?: return
        val vf = LocalFileSystem.getInstance().findFileByPath("$base/src/main.lm")
        if (vf != null) {
            FileEditorManager.getInstance(project).openFile(vf, true)
        }
    }
}

private class LumiCreateProjectDialog : DialogWrapper(null) {
    private val projectNameField = JTextField("my_app", 24)
    private val packageNameField = JTextField("my_app", 24)
    private val locationField = TextFieldWithBrowseButton()

    init {
        title = "New Lumi Project"
        locationField.text = defaultProjectsDir()
        locationField.addBrowseFolderListener(
            "Select Parent Directory",
            "Choose where the new Lumi project folder will be created",
            null,
            null,
        )
        init()
    }

    val projectPath: String
        get() = File(locationField.text.trim(), projectNameField.text.trim()).absolutePath

    val packageName: String
        get() = packageNameField.text.trim().ifEmpty { projectNameField.text.trim() }

    override fun createCenterPanel(): JComponent =
        FormBuilder.createFormBuilder()
            .addLabeledComponent("Project name:", projectNameField)
            .addLabeledComponent("Package name:", packageNameField)
            .addLabeledComponent("Location:", locationField)
            .panel

    override fun doValidate(): ValidationInfo? {
        val name = projectNameField.text.trim()
        val loc = locationField.text.trim()
        if (name.isEmpty()) return ValidationInfo("Project name is required", projectNameField)
        val pkgErr = LumiProjectScaffold.validatePackageName(packageName)
        if (pkgErr != null) return ValidationInfo(pkgErr, packageNameField)
        if (loc.isEmpty()) return ValidationInfo("Location is required", locationField)
        val dir = File(loc, name)
        if (dir.exists()) {
            return ValidationInfo("Directory already exists: ${dir.path}", projectNameField)
        }
        return null
    }

    override fun doOKAction() {
        syncPackageFromName()
        super.doOKAction()
    }

    private fun syncPackageFromName() {
        if (packageNameField.text.isBlank() || packageNameField.text == sanitize(projectNameField.text)) {
            packageNameField.text = sanitize(projectNameField.text)
        }
    }

    private fun sanitize(name: String): String = name.trim().replace(' ', '_')

    private fun defaultProjectsDir(): String {
        val home = System.getProperty("user.home")
        val idea = File(home, "IdeaProjects")
        if (idea.isDirectory) return idea.absolutePath
        return File(home, "Projects").absolutePath
    }
}
