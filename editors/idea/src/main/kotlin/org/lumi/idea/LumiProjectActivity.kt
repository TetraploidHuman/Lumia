package org.lumi.idea

import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.FileEditorManagerListener
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClientManager

class LumiProjectActivity : ProjectActivity {
    override suspend fun execute(project: Project) {
        if (project.isDefault) return
        kick(project, "projectActivity")
        project.messageBus.connect().subscribe(
            FileEditorManagerListener.FILE_EDITOR_MANAGER,
            object : FileEditorManagerListener {
                override fun fileOpened(source: FileEditorManager, file: VirtualFile) {
                    if (LumiPaths.isLumiFile(file)) {
                        kick(project, "fileOpened ${file.path}")
                    }
                }
            },
        )
    }

    companion object {
        private val LOG = Logger.getInstance(LumiProjectActivity::class.java)

        fun kick(project: Project, reason: String) {
            if (project.isDisposed) return
            LOG.info("Lumi LSP startClientsIfNeeded ($reason)")
            LspClientManager.getInstance(project)
                .startClientsIfNeeded(LumiLspServerSupportProvider::class.java)
        }
    }
}
