package org.lumi.idea

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.ProjectWideLspClientDescriptor

class LumiLspClientDescriptor(project: Project) :
    ProjectWideLspClientDescriptor(project, "Lumi") {

    override fun isSupportedFile(file: VirtualFile): Boolean =
        LumiPaths.isLumiFile(file)

    override fun getLanguageId(file: VirtualFile): String = "lumi"

    override fun createCommandLine(): GeneralCommandLine {
        val path = LumiPaths.resolveLumi(project)
        return LumiPaths.applyRuntimeEnvironment(
            GeneralCommandLine(path, "lsp"),
            project,
            path,
        )
            .withCharset(Charsets.UTF_8)
            .withParentEnvironmentType(GeneralCommandLine.ParentEnvironmentType.CONSOLE)
    }
}
