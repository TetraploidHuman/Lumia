package org.lumia.idea

import com.intellij.openapi.options.Configurable
import com.intellij.ui.components.JBCheckBox
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent
import javax.swing.JPanel

class LumiaSettingsConfigurable : Configurable {
    private var pathField: JBTextField? = null
    private var autoParallelBox: JBCheckBox? = null

    override fun getDisplayName(): String = "Lumia"

    override fun createComponent(): JComponent {
        val settings = LumiaSettings.getInstance()
        val field = JBTextField(settings.lspPath)
        pathField = field
        val parallel = JBCheckBox("Auto-parallelize List.map / fold in LSP", settings.autoParallel)
        autoParallelBox = parallel
        return FormBuilder.createFormBuilder()
            .addLabeledComponent(JBLabel("Path to lumia:"), field, 1, false)
            .addComponent(parallel, 1)
            .addComponentFillVertically(JPanel(), 0)
            .panel
    }

    override fun isModified(): Boolean {
        val s = LumiaSettings.getInstance()
        return pathField?.text != s.lspPath
            || autoParallelBox?.isSelected != s.autoParallel
    }

    override fun apply() {
        val s = LumiaSettings.getInstance()
        s.lspPath = pathField?.text?.trim().orEmpty().ifBlank { "lumia" }
        s.autoParallel = autoParallelBox?.isSelected ?: true
    }

    override fun reset() {
        val s = LumiaSettings.getInstance()
        pathField?.text = s.lspPath
        autoParallelBox?.isSelected = s.autoParallel
    }

    override fun disposeUIResources() {
        pathField = null
        autoParallelBox = null
    }
}
