package org.lumia.idea

import com.intellij.execution.actions.ConfigurationContext
import com.intellij.execution.actions.LazyRunConfigurationProducer
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.openapi.util.Ref
import com.intellij.psi.PsiElement
import java.io.File

private fun isLumiaFile(context: ConfigurationContext): Boolean {
    val file = context.location?.virtualFile ?: return false
    return LumiaPaths.isLumiaFile(file)
}

private fun entryFor(context: ConfigurationContext): String? {
    val file = context.location?.virtualFile ?: return null
    return LumiaPaths.resolveProjectEntry(context.project, file)
}

class LumiaCheckConfigurationProducer : LazyRunConfigurationProducer<LumiaRunConfiguration>() {
    override fun getConfigurationFactory(): ConfigurationFactory =
        LumiaRunConfigurationType.checkFactory()

    override fun setupConfigurationFromContext(
        configuration: LumiaRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        if (!isLumiaFile(context)) return false
        val entry = entryFor(context) ?: return false
        configuration.filePath = entry
        configuration.mode = LumiaRunMode.CHECK
        configuration.name = "Check: ${File(entry).nameWithoutExtension}"
        return true
    }

    override fun isConfigurationFromContext(
        configuration: LumiaRunConfiguration,
        context: ConfigurationContext,
    ): Boolean {
        if (configuration.mode != LumiaRunMode.CHECK) return false
        val entry = entryFor(context) ?: return false
        return configuration.filePath == entry
    }
}

class LumiaBuildConfigurationProducer : LazyRunConfigurationProducer<LumiaRunConfiguration>() {
    override fun getConfigurationFactory(): ConfigurationFactory =
        LumiaRunConfigurationType.buildFactory()

    override fun setupConfigurationFromContext(
        configuration: LumiaRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        if (!isLumiaFile(context)) return false
        val entry = entryFor(context) ?: return false
        configuration.filePath = entry
        configuration.mode = LumiaRunMode.BUILD_RUN
        configuration.name = "Build: ${File(entry).nameWithoutExtension}"
        return true
    }

    override fun isConfigurationFromContext(
        configuration: LumiaRunConfiguration,
        context: ConfigurationContext,
    ): Boolean {
        if (configuration.mode != LumiaRunMode.BUILD_RUN) return false
        val entry = entryFor(context) ?: return false
        return configuration.filePath == entry
    }
}
