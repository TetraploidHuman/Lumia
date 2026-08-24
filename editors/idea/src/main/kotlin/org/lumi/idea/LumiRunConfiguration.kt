package org.lumi.idea

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

enum class LumiRunMode(val id: String, val label: String) {
    CHECK("check", "Check (lumi check)"),
    BUILD_RUN("build", "Build & Run (lumi build)"),
    ;

    companion object {
        fun fromId(id: String?): LumiRunMode =
            entries.firstOrNull { it.id == id } ?: CHECK
    }
}

class LumiRunConfigurationType : ConfigurationTypeBase(
    ID,
    "Lumi",
    "Check or build & run a Lumi (.lm) file",
    NotNullLazyValue.createValue { LumiIcons.FILE },
) {
    init {
        addFactory(LumiCheckFactory(this))
        addFactory(LumiBuildFactory(this))
    }

    companion object {
        const val ID = "LumiRunConfiguration"

        fun getInstance(): LumiRunConfigurationType =
            ConfigurationTypeUtil.findConfigurationType(LumiRunConfigurationType::class.java)

        fun checkFactory(): ConfigurationFactory =
            getInstance().configurationFactories.first { it.id == LumiCheckFactory.ID }

        fun buildFactory(): ConfigurationFactory =
            getInstance().configurationFactories.first { it.id == LumiBuildFactory.ID }
    }
}

class LumiCheckFactory(type: LumiRunConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = ID
    override fun getName(): String = "Check"

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        LumiRunConfiguration(project, this, "Lumi check").also {
            it.mode = LumiRunMode.CHECK
            it.filePath = LumiPaths.resolveProjectEntry(project).orEmpty()
        }

    companion object {
        const val ID = "LumiCheck"
    }
}

class LumiBuildFactory(type: LumiRunConfigurationType) : ConfigurationFactory(type) {
    override fun getId(): String = ID
    override fun getName(): String = "Build & Run"

    override fun createTemplateConfiguration(project: Project): RunConfiguration =
        LumiRunConfiguration(project, this, "Lumi build").also {
            it.mode = LumiRunMode.BUILD_RUN
            it.filePath = LumiPaths.resolveProjectEntry(project).orEmpty()
        }

    companion object {
        const val ID = "LumiBuild"
    }
}

class LumiRunConfiguration(
    project: Project,
    factory: ConfigurationFactory,
    name: String,
) : LocatableConfigurationBase<Element>(project, factory, name) {

    var filePath: String = ""
    var mode: LumiRunMode = when (factory.id) {
        LumiBuildFactory.ID -> LumiRunMode.BUILD_RUN
        else -> LumiRunMode.CHECK
    }

    override fun getConfigurationEditor(): SettingsEditor<out RunConfiguration> =
        LumiRunSettingsEditor()

    override fun checkConfiguration() {
        if (filePath.isBlank()) {
            throw RuntimeConfigurationError("Lumi file path is empty")
        }
        if (!File(filePath).isFile) {
            throw RuntimeConfigurationError("Lumi file not found: $filePath")
        }
        val lumi = LumiPaths.resolveLumi(project)
        if (!File(lumi).canExecute()) {
            throw RuntimeConfigurationError(
                "lumi not found ($lumi). Build with: source scripts/env.sh && cargo build -p lumi --release",
            )
        }
    }

    override fun getState(executor: Executor, environment: ExecutionEnvironment): RunProfileState {
        return object : CommandLineState(environment) {
            override fun startProcess(): ProcessHandler {
                val lumi = LumiPaths.resolveLumi(project)
                val workDir = project.basePath ?: File(filePath).parent ?: "."
                val commandLine = when (mode) {
                    LumiRunMode.CHECK ->
                        GeneralCommandLine(lumi, "check", filePath)
                    LumiRunMode.BUILD_RUN -> {
                        val stem = File(filePath).nameWithoutExtension
                            .replace(Regex("[^A-Za-z0-9_]"), "_")
                            .ifBlank { "out" }
                        val outDir = File(workDir, "target/lumi")
                        outDir.mkdirs()
                        val out = File(outDir, stem).path
                        // Build then exec: no shell — run build first, then return binary handler.
                        val build = LumiPaths.applyRuntimeEnvironment(
                            GeneralCommandLine(lumi, "build", filePath, "-o", out),
                            project,
                            lumi,
                        )
                            .withWorkDirectory(workDir)
                            .withCharset(Charsets.UTF_8)
                            .withParentEnvironmentType(GeneralCommandLine.ParentEnvironmentType.CONSOLE)
                        val buildProc = build.createProcess()
                        val code = buildProc.waitFor()
                        val err = buildProc.errorStream.bufferedReader().readText()
                        val outText = buildProc.inputStream.bufferedReader().readText()
                        if (code != 0) {
                            throw ExecutionException(
                                "lumi build failed (exit $code)\n$outText$err",
                            )
                        }
                        GeneralCommandLine(out)
                    }
                }
                LumiPaths.applyRuntimeEnvironment(commandLine, project, lumi)
                    .withWorkDirectory(workDir)
                    .withCharset(Charsets.UTF_8)
                    .withParentEnvironmentType(GeneralCommandLine.ParentEnvironmentType.CONSOLE)
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
        mode = LumiRunMode.fromId(element.getAttributeValue("mode"))
    }
}

class LumiRunSettingsEditor : SettingsEditor<LumiRunConfiguration>() {
    private val fileField = JBTextField()
    private val modeBox = JComboBox(LumiRunMode.entries.toTypedArray()).apply {
        renderer = object : javax.swing.DefaultListCellRenderer() {
            override fun getListCellRendererComponent(
                list: javax.swing.JList<*>?,
                value: Any?,
                index: Int,
                isSelected: Boolean,
                cellHasFocus: Boolean,
            ): java.awt.Component {
                val c = super.getListCellRendererComponent(list, value, index, isSelected, cellHasFocus)
                if (value is LumiRunMode) text = value.label
                return c
            }
        }
    }

    override fun resetEditorFrom(s: LumiRunConfiguration) {
        fileField.text = s.filePath
        modeBox.selectedItem = s.mode
    }

    override fun applyEditorTo(s: LumiRunConfiguration) {
        s.filePath = fileField.text.trim()
        s.mode = modeBox.selectedItem as? LumiRunMode ?: LumiRunMode.CHECK
    }

    override fun createEditor(): JComponent =
        FormBuilder.createFormBuilder()
            .addLabeledComponent("File:", fileField)
            .addLabeledComponent("Mode:", modeBox)
            .addComponentFillVertically(JPanel(), 0)
            .panel
}
