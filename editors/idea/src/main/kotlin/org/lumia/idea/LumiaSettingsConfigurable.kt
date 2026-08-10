package org.lumia.idea

import com.intellij.openapi.options.Configurable
import com.intellij.ui.components.JBLabel
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import javax.swing.JComponent
import javax.swing.JPanel

class LumiaSettingsConfigurable : Configurable {
    private var pathField: JBTextField? = null

    override fun getDisplayName(): String = "Lumia"

    override fun createComponent(): JComponent {
        val field = JBTextField(LumiaSettings.getInstance().lspPath)
        pathField = field
        return FormBuilder.createFormBuilder()
            .addLabeledComponent(JBLabel("Path to lumia:"), field, 1, false)
            .addComponentFillVertically(JPanel(), 0)
            .panel
    }

    override fun isModified(): Boolean =
        pathField?.text != LumiaSettings.getInstance().lspPath

    override fun apply() {
        LumiaSettings.getInstance().lspPath =
            pathField?.text?.trim().orEmpty().ifBlank { "lumia" }
    }

    override fun reset() {
        pathField?.text = LumiaSettings.getInstance().lspPath
    }

    override fun disposeUIResources() {
        pathField = null
    }
}
