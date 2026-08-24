package org.lumi.idea

import com.intellij.openapi.options.Configurable
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent
import javax.swing.JPanel

class LumiSettingsConfigurable : Configurable {
    private var pathField: JBTextField? = null

    override fun getDisplayName(): String = "Lumi"

    override fun createComponent(): JComponent {
        val field = JBTextField(LumiSettings.getInstance().lspPath)
        pathField = field
        return FormBuilder.createFormBuilder()
            .addLabeledComponent(JBLabel("Path to lumi:"), field, 1, false)
            .addComponentFillVertically(JPanel(), 0)
            .panel
    }

    override fun isModified(): Boolean =
        pathField?.text != LumiSettings.getInstance().lspPath

    override fun apply() {
        LumiSettings.getInstance().lspPath =
            pathField?.text?.trim().orEmpty().ifBlank { "lumi" }
    }

    override fun reset() {
        pathField?.text = LumiSettings.getInstance().lspPath
    }

    override fun disposeUIResources() {
        pathField = null
    }
}
