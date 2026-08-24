package org.lumi.idea

import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.executors.DefaultRunExecutor
import com.intellij.execution.impl.ConsoleViewImpl
import com.intellij.execution.process.ProcessHandlerFactory
import com.intellij.execution.process.ProcessTerminatedListener
import com.intellij.execution.ui.RunContentDescriptor
import com.intellij.execution.ui.RunContentManager
import com.intellij.openapi.actionSystem.ActionManager
import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.fileEditor.FileDocumentManager
import com.intellij.openapi.project.DumbAware
import com.intellij.openapi.project.Project
import com.intellij.openapi.ui.Messages
import com.intellij.platform.lsp.api.LspClientManager
import java.io.File

private fun currentLmPath(e: AnActionEvent): String? {
    val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return null
    if (!LumiPaths.isLumiFile(file)) return null
    FileDocumentManager.getInstance().saveAllDocuments()
    return file.path
}

private fun runInConsole(project: Project, title: String, cmd: GeneralCommandLine) {
    val handler = ProcessHandlerFactory.getInstance().createColoredProcessHandler(cmd)
    ProcessTerminatedListener.attach(handler)
    val console = ConsoleViewImpl(project, true)
    console.attachToProcess(handler)
    handler.startNotify()
    RunContentManager.getInstance(project).showRunContent(
        DefaultRunExecutor.getRunExecutorInstance(),
        RunContentDescriptor(console, handler, console.component, title),
    )
}

class LumiCheckFileAction : AnAction(), DumbAware {
    override fun actionPerformed(e: AnActionEvent) {
        val path = currentLmPath(e) ?: return
        val project = e.project ?: return
        val cmd = GeneralCommandLine(LumiPaths.resolveLumi(), "check", path)
            .withWorkDirectory(File(path).parent)
            .withCharset(Charsets.UTF_8)
            .withEnvironment("PATH", LumiPaths.pathWithExtras())
        runInConsole(project, "Lumi Check", cmd)
    }
}

class LumiBuildFileAction : AnAction(), DumbAware {
    override fun actionPerformed(e: AnActionEvent) {
        val path = currentLmPath(e) ?: return
        val project = e.project ?: return
        val out = File(File(path).parent, File(path).nameWithoutExtension).path
        val cmd = GeneralCommandLine(LumiPaths.resolveLumi(), "build", path, "-o", out)
            .withWorkDirectory(File(path).parent)
            .withCharset(Charsets.UTF_8)
            .withEnvironment("PATH", LumiPaths.pathWithExtras())
        runInConsole(project, "Lumi Build", cmd)
    }
}

class LumiFormatFileAction : AnAction(), DumbAware {
    override fun actionPerformed(e: AnActionEvent) {
        val file = e.getData(CommonDataKeys.PSI_FILE) ?: return
        if (file.fileType !== LumiFileType.INSTANCE) return
        ActionManager.getInstance().getAction("ReformatCode")?.actionPerformed(e)
    }
}

class LumiRestartLspAction : AnAction(), DumbAware {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.project ?: return
        restartLsp(project)
        Messages.showInfoMessage(project, "Lumi language server restart requested.", "Lumi")
    }
}

fun restartLsp(project: Project) {
    try {
        val mgr = LspClientManager.getInstance(project)
        // Best-effort stop; API surface varies across IDEA builds.
        mgr.javaClass.methods
            .firstOrNull { it.name == "stopClients" && it.parameterCount == 1 }
            ?.invoke(mgr, LumiLspServerSupportProvider::class.java)
    } catch (_: Throwable) {
        // ignore — startClientsIfNeeded is idempotent for project-wide clients
    }
    LumiProjectActivity.kick(project, "manualRestart")
}
