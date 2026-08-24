package org.lumi.idea

import com.intellij.openapi.diagnostic.Logger
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.platform.lsp.api.LspClient
import com.intellij.platform.lsp.api.LspIntegrationProvider
import com.intellij.platform.lsp.api.lsWidget.LspClientWidgetItem

class LumiLspServerSupportProvider : LspIntegrationProvider {
    override fun fileOpened(
        project: Project,
        file: VirtualFile,
        clientStarter: LspIntegrationProvider.LspClientStarter,
    ) {
        if (LumiPaths.isLumiFile(file)) {
            LOG.info("Lumi LSP fileOpened ${file.path}")
            clientStarter.ensureClientStarted(LumiLspClientDescriptor(project))
        }
    }

    override fun createWidgetItem(
        lspClient: LspClient,
        currentFile: VirtualFile?,
    ): LspClientWidgetItem =
        LspClientWidgetItem(
            lspClient,
            currentFile,
            LumiIcons.FILE,
            LumiSettingsConfigurable::class.java,
        )

    companion object {
        private val LOG = Logger.getInstance(LumiLspServerSupportProvider::class.java)
    }
}
