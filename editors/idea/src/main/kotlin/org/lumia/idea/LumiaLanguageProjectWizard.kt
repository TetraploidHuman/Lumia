package org.lumia.idea

import com.intellij.ide.wizard.AbstractNewProjectWizardStep
import com.intellij.ide.wizard.NewProjectWizardBaseData
import com.intellij.ide.wizard.NewProjectWizardStep
import com.intellij.ide.wizard.language.LanguageGeneratorNewProjectWizard
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.LocalFileSystem
import com.intellij.ui.dsl.builder.Panel
import java.io.File

/**
 * Sidebar entry under **New Project** (with Java/Kotlin) on a local/monolithic IDE.
 *
 * Note: JetBrains Client (Remote Development) renders this wizard on the thin client;
 * backend-only plugins may not appear there — use **New → Lumia Project** after opening
 * a remote workspace, or run `scripts/new_lumia_project.sh` on the remote host.
 */
class LumiaLanguageProjectWizard : LanguageGeneratorNewProjectWizard {
    override val name: String = "Lumia"
    override val icon = LumiaIcons.FILE
    override val ordinal: Int = 250

    override fun createStep(parent: NewProjectWizardStep): NewProjectWizardStep =
        LumiaSetupStep(parent)
}

/** Writes `Lumia.toml` + `src/main.lm` once the wizard creates the project directory. */
internal class LumiaSetupStep(
    parent: NewProjectWizardStep,
) : AbstractNewProjectWizardStep(parent),
    NewProjectWizardBaseData by parent as NewProjectWizardBaseData {

    override fun setupUI(builder: Panel) {
        // Name / location / Git come from platform steps in the parent chain.
    }

    override fun setupProject(project: Project) {
        val pkg = packageNameFromProject(name)
        val root = project.basePath?.let { File(it) } ?: File(path, name)
        if (!root.isDirectory) {
            LumiaProjectScaffold.createOnDisk(root.absolutePath, pkg)
        } else {
            writeScaffold(root, pkg)
        }
        val vf = LocalFileSystem.getInstance().refreshAndFindFileByIoFile(root)
        vf?.refresh(true, true)
        LumiaProjectActivity.kick(project, "newProjectWizard")
    }

    private fun writeScaffold(root: File, pkg: String) {
        val moduleName = LumiaProjectScaffold.moduleNameFromPackage(pkg)
        if (!File(root, "Lumia.toml").exists()) {
            File(root, "Lumia.toml").writeText(LumiaProjectScaffold.renderToml(pkg))
        }
        val src = File(root, "src")
        if (!src.exists()) {
            src.mkdirs()
        }
        val main = File(src, "main.lm")
        if (!main.exists()) {
            main.writeText(LumiaProjectScaffold.renderMainLm(moduleName))
        }
    }

    private fun packageNameFromProject(projectName: String): String {
        val n = projectName.trim().replace(' ', '_')
        return if (LumiaProjectScaffold.validatePackageName(n) == null) {
            n
        } else {
            "lumia_app"
        }
    }
}
