package org.lumia.idea

import com.intellij.ide.actions.CreateFileFromTemplateAction
import com.intellij.ide.actions.CreateFileFromTemplateDialog
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiDirectory

class LumiaNewFileAction :
    CreateFileFromTemplateAction("Lumia File", "Create a new Lumia source file", LumiaIcons.FILE),
    DumbAware {

    override fun buildDialog(
        project: Project,
        directory: PsiDirectory,
        builder: CreateFileFromTemplateDialog.Builder,
    ) {
        builder
            .setTitle("New Lumia File")
            .addKind("Lumia file", LumiaIcons.FILE, "Lumia File")
    }

    override fun getActionName(directory: PsiDirectory?, newName: String, templateName: String?): String =
        "Lumia File"
}
