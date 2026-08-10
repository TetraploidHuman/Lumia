package org.lumia.idea

import com.intellij.execution.ExecutionException
import com.intellij.execution.Executor
import com.intellij.execution.configurations.CommandLineState
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.execution.configurations.ConfigurationTypeBase
import com.intellij.execution.configurations.ConfigurationTypeUtil
import com.intellij.execution.configurations.GeneralCommandLine
import com.intellij.execution.configurations.LocatableConfigurationBase
import com.intellij.execution.configurations.RunConfiguration
import com.intellij.execution.configurations.RunProfileState
import com.intellij.execution.configurations.RuntimeConfigurationError
import com.intellij.execution.process.ProcessHandler
import com.intellij.execution.process.ProcessHandlerFactory
import com.intellij.execution.process.ProcessTerminatedListener
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.NotNullLazyValue
import com.intellij.ui.components.JBTextField
import com.intellij.util.ui.FormBuilder
import org.jdom.Element
import java.io.File
import javax.swing.JComboBox
import javax.swing.JComponent
import javax.swing.JPanel

enum class LumiaRunMode(val id: String, val label: String) {
    CHECK("check", "Check (lumia check)"),
    BUILD_RUN("build", "Build & Run (lumia build)"),
    ;

    companion object {
        fun fromId(id: String?): LumiaRunMode =
            entries.firstOrNull { it.id == id } ?: CHECK
    }
}

class LumiaRunConfigurationType : ConfigurationTypeBase(
    ID,
    "Lumia",
    "Check or build & run a Lumia (.lm) file",
    NotNullLazyValue.createValue { LumiaIcons.FILE },
) {
    init {
        addFactory(LumiaCheckFactory(this))
        addFactory(LumiaBuildFactory(this))
    }

    companion object {
        const val ID = "LumiaRunConfiguration"

        fun getInstance(): LumiaRunConfigurationType =
            ConfigurationTypeUtil.findConfigurationType(LumiaRunConfigurationType::class.java)

        fun checkFactory(): ConfigurationFactory =
            getInstance().configurationFactories.first { it.id == LumiaCheckFactory.ID }

        fun buildFactory(): ConfigurationFactory =
            getInstance().configurationFactories.first { it.id == LumiaBuildFactory.ID }
    }
}

class LumiaCheckFactory(type: LumiaRunConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = ID
    override fun getName(): String = "Check"

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        LumiaRunConfiguration(project, this, "Lumia check").also {
            it.mode = LumiaRunMode.CHECK
            it.filePath = LumiaPaths.resolveProjectEntry(project).orEmpty()
        }

    companion object {
        const val ID = "LumiaCheck"
    }
}

class LumiaBuildFactory(type: LumiaRunConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = ID
    override fun getName(): String = "Build & Run"

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        LumiaRunConfiguration(project, this, "Lumia build").also {
            it.mode = LumiaRunMode.BUILD_RUN
            it.filePath = LumiaPaths.resolveProjectEntry(project).orEmpty()
        }

    companion object {
        const val ID = "LumiaBuild"
    }
}

class LumiaRunConfiguration(
    project: Project,
    factory: ConfigurationFactory,
    name: String,
) : LocatableConfigurationBase<Element>(project, factory, name) {

    var filePath: String = ""
    var mode: LumiaRunMode = when (factory.id) {
        LumiaBuildFactory.ID -> LumiaRunMode.BUILD_RUN
        else -> LumiaRunMode.CHECK
    }

    override fun getConfigurationEditor(): SettingsEditor<out RunConfiguration> =
        LumiaRunSettingsEditor()

    override fun checkConfiguration() {
        if (filePath.isBlank()) {
            throw RuntimeConfigurationError("Lumia file path is empty")
        }
        if (!File(filePath).isFile) {
            throw RuntimeConfigurationError("Lumia file not found: $filePath")
        }
        val lumia = LumiaPaths.resolveLumia()
        if (!File(lumia).canExecute() && lumia.contains(File.separator)) {
            throw RuntimeConfigurationError(
                "lumia not found ($lumia). Build with: cargo build -p lumia --release",
            )
        }
    }

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState {
        return object : CommandLineState(environment) {
            override fun startProcess(): ProcessHandler {
                val lumia = LumiaPaths.resolveLumia()
                val workDir = project.basePath ?: File(filePath).parent ?: "."
                val commandLine = when (mode) {
                    LumiaRunMode.CHECK ->
                        GeneralCommandLine(lumia, "check", filePath)
                    LumiaRunMode.BUILD_RUN -> {
                        val stem = File(filePath).nameWithoutExtension
                            .replace(Regex("[^A-Za-z0-9_]"), "_")
                            .ifBlank { "out" }
                        val outDir = File(workDir, "target/lumia")
                        outDir.mkdirs()
                        val out = File(outDir, stem).path
                        // Build then exec: no shell — run build first, then return binary handler.
                        val build = GeneralCommandLine(lumia, "build", filePath, "-o", out)
                            .withWorkDirectory(workDir)
                            .withCharset(Charsets.UTF_8)
                            .withParentEnvironmentType(GeneralCommandLine.ParentEnvironmentType.CONSOLE)
                            .withEnvironment("PATH", LumiaPaths.pathWithExtras())
                        val buildProc = build.createProcess()
                        val code = buildProc.waitFor()
                        val err = buildProc.errorStream.bufferedReader().readText()
                        val outText = buildProc.inputStream.bufferedReader().readText()
                        if (code != 0) {
                            throw ExecutionException(
                                "lumia build failed (exit $code)\n$outText$err",
                            )
                        }
                        GeneralCommandLine(out)
                    }
                }
                commandLine
                    .withWorkDirectory(workDir)
                    .withCharset(Charsets.UTF_8)
                    .withParentEnvironmentType(GeneralCommandLine.ParentEnvironmentType.CONSOLE)
                    .withEnvironment("PATH", LumiaPaths.pathWithExtras())
                val handler = ProcessHandlerFactory.getInstance()
                    .createColoredProcessHandler(commandLine)
                ProcessTerminatedListener.attach(handler)
                return handler
            }
        }
    }

    override fun writeExternal(element: Element) {
        super.writeExternal(element)
        element.setAttribute("filePath", filePath)
        element.setAttribute("mode", mode.id)
    }

    override fun readExternal(element: Element) {
        super.readExternal(element)
        filePath = element.getAttributeValue("filePath").orEmpty()
        mode = LumiaRunMode.fromId(element.getAttributeValue("mode"))
    }
}

class LumiaRunSettingsEditor : SettingsEditor<LumiaRunConfiguration>() {
    private val fileField = JBTextField()
    private val modeBox = JComboBox(LumiaRunMode.entries.toTypedArray()).apply {
        renderer = object : javax.swing.DefaultListCellRenderer() {
            override fun getListCellRendererComponent(
                list: javax.swing.JList<*>?,
                value: Any?,
                index: Int,
                isSelected: Boolean,
                cellHasFocus: Boolean,
            ): java.awt.Component {
                val c = super.getListCellRendererComponent(list, value, index, isSelected, cellHasFocus)
                if (value is LumiaRunMode) text = value.label
                return c
            }
        }
    }

    override fun resetEditorFrom(s: LumiaRunConfiguration) {
        fileField.text = s.filePath
        modeBox.selectedItem = s.mode
    }

    override fun applyEditorTo(s: LumiaRunConfiguration) {
        s.filePath = fileField.text.trim()
        s.mode = modeBox.selectedItem as? LumiaRunMode ?: LumiaRunMode.CHECK
    }

    override fun createEditor(): JComponent =
        FormBuilder.createFormBuilder()
            .addLabeledComponent("File:", fileField)
            .addLabeledComponent("Mode:", modeBox)
            .addComponentFillVertically(JPanel(), 0)
            .panel
}
