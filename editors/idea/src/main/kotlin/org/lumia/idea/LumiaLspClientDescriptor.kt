package org.lumia.idea

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor

class LumiaLspClientDescriptor(project: Project) :
    ProjectWideLspClientDescriptor(project, "Lumia") {

    override fun isSupportedFile(file: VirtualFile): Boolean =
        LumiaPaths.isLumiaFile(file)

    override fun getLanguageId(file: VirtualFile): String = "lumia"

    override fun createCommandLine(): GeneralCommandLine {
        val path = LumiaPaths.resolveLumia()
        return GeneralCommandLine(path, "lsp")
            .withCharset(Charsets.UTF_8)
            .withParentEnvironmentType(GeneralCommandLine.ParentEnvironmentType.CONSOLE)
            .withEnvironment("PATH", LumiaPaths.pathWithExtras())
    }
}
