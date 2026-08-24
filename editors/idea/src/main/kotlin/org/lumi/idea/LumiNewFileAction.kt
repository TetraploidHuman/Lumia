package org.lumi.idea

import com.intellij.ide.actions.CreateFileFromTemplateAction
import com.intellij.ide.actions.CreateFileFromTemplateDialog
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiDirectory

class LumiNewFileAction :
    CreateFileFromTemplateAction("Lumi File", "Create a new Lumi source file", LumiIcons.FILE),
    DumbAware {

    override fun buildDialog(
        project: Project,
        directory: PsiDirectory,
        builder: CreateFileFromTemplateDialog.Builder,
    ) {
        builder
            .setTitle("New Lumi File")
            .addKind("Lumi file", LumiIcons.FILE, "Lumi File")
    }

    override fun getActionName(directory: PsiDirectory?, newName: String, templateName: String?): String =
        "Lumi File"
}
