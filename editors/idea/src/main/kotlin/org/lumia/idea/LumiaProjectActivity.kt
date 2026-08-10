package org.lumia.idea

import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.fileEditor.FileEditorManager
import com.intellij.openapi.fileEditor.FileEditorManagerListener
import com.intellij.openapi.project.Project
import com.intellij.openapi.startup.ProjectActivity
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClientManager

class LumiaProjectActivity : ProjectActivity {
    override suspend fun execute(project: Project) {
        if (project.isDefault) return
        kick(project, "projectActivity")
        project.messageBus.connect().subscribe(
            FileEditorManagerListener.FILE_EDITOR_MANAGER,
            object : FileEditorManagerListener {
                override fun fileOpened(source: FileEditorManager, file: VirtualFile) {
                    if (LumiaPaths.isLumiaFile(file)) {
                        kick(project, "fileOpened ${file.path}")
                    }
                }
            },
        )
    }

    companion object {
        private val LOG = Logger.getInstance(LumiaProjectActivity::class.java)

        fun kick(project: Project, reason: String) {
            if (project.isDisposed) return
            LOG.info("Lumia LSP startClientsIfNeeded ($reason)")
            LspClientManager.getInstance(project)
                .startClientsIfNeeded(LumiaLspServerSupportProvider::class.java)
        }
    }
}
