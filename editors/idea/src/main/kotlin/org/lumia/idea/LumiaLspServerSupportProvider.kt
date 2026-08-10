package org.lumia.idea

import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspIntegrationProvider
import com.intellij.platform.lsp.api.lsWidget.LspClientWidgetItem

class LumiaLspServerSupportProvider : LspIntegrationProvider {
    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (LumiaPaths.isLumiaFile(file)) {
            LOG.info("Lumia LSP fileOpened ${file.path}")
            clientStarter.ensureClientStarted(LumiaLspClientDescriptor(project))
        }
    }

    override fun createWidgetItem(
        lspClient: LspClient,
        currentFile: VirtualFile?,
    ): LspClientWidgetItem =
        LspClientWidgetItem(
            lspClient,
            currentFile,
            LumiaIcons.FILE,
            LumiaSettingsConfigurable::class.java,
        )

    companion object {
        private val LOG = Logger.getInstance(LumiaLspServerSupportProvider::class.java)
    }
}
