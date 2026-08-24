package org.lumi.idea

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
 * backend-only plugins may not appear there — use **New → Lumi Project** after opening
 * a remote workspace, or run `scripts/new_lumi_project.sh` on the remote host.
 */
class LumiLanguageProjectWizard : LanguageGeneratorNewProjectWizard {
    override val name: String = "Lumi"
    override val icon = LumiIcons.FILE
    override val ordinal: Int = 250

    override fun createStep(parent: NewProjectWizardStep): NewProjectWizardStep =
        LumiSetupStep(parent)
}

/** Writes `Lumi.toml` + `src/main.lm` once the wizard creates the project directory. */
internal class LumiSetupStep(
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
            LumiProjectScaffold.createOnDisk(root.absolutePath, pkg)
        } else {
            writeScaffold(root, pkg)
        }
        val vf = LocalFileSystem.getInstance().refreshAndFindFileByIoFile(root)
        vf?.refresh(true, true)
        LumiProjectActivity.kick(project, "newProjectWizard")
    }

    private fun writeScaffold(root: File, pkg: String) {
        val moduleName = LumiProjectScaffold.moduleNameFromPackage(pkg)
        if (!File(root, "Lumi.toml").exists()) {
            File(root, "Lumi.toml").writeText(LumiProjectScaffold.renderToml(pkg))
        }
        val src = File(root, "src")
        if (!src.exists()) {
            src.mkdirs()
        }
        val main = File(src, "main.lm")
        if (!main.exists()) {
            main.writeText(LumiProjectScaffold.renderMainLm(moduleName))
        }
    }

    private fun packageNameFromProject(projectName: String): String {
        val n = projectName.trim().replace(' ', '_')
        return if (LumiProjectScaffold.validatePackageName(n) == null) {
            n
        } else {
            "lumi_app"
        }
    }
}
