package org.lumi.idea

import com.intellij.execution.actions.ConfigurationContext
import com.intellij.execution.actions.LazyRunConfigurationProducer
import com.intellij.execution.configurations.ConfigurationFactory
import com.intellij.openapi.util.Ref
import com.intellij.psi.PsiElement
import java.io.File

private fun isLumiFile(context: ConfigurationContext): Boolean {
    val file = context.location?.virtualFile ?: return false
    return LumiPaths.isLumiFile(file)
}

private fun entryFor(context: ConfigurationContext): String? {
    val file = context.location?.virtualFile ?: return null
    return LumiPaths.resolveProjectEntry(context.project, file)
}

class LumiCheckConfigurationProducer : LazyRunConfigurationProducer<LumiRunConfiguration>() {
    override fun getConfigurationFactory(): ConfigurationFactory =
        LumiRunConfigurationType.checkFactory()

    override fun setupConfigurationFromContext(
        configuration: LumiRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        if (!isLumiFile(context)) return false
        val entry = entryFor(context) ?: return false
        configuration.filePath = entry
        configuration.mode = LumiRunMode.CHECK
        configuration.name = "Check: ${File(entry).nameWithoutExtension}"
        return true
    }

    override fun isConfigurationFromContext(
        configuration: LumiRunConfiguration,
        context: ConfigurationContext,
    ): Boolean {
        if (configuration.mode != LumiRunMode.CHECK) return false
        val entry = entryFor(context) ?: return false
        return configuration.filePath == entry
    }
}

class LumiBuildConfigurationProducer : LazyRunConfigurationProducer<LumiRunConfiguration>() {
    override fun getConfigurationFactory(): ConfigurationFactory =
        LumiRunConfigurationType.buildFactory()

    override fun setupConfigurationFromContext(
        configuration: LumiRunConfiguration,
        context: ConfigurationContext,
        sourceElement: Ref<PsiElement>,
    ): Boolean {
        if (!isLumiFile(context)) return false
        val entry = entryFor(context) ?: return false
        configuration.filePath = entry
        configuration.mode = LumiRunMode.BUILD_RUN
        configuration.name = "Build: ${File(entry).nameWithoutExtension}"
        return true
    }

    override fun isConfigurationFromContext(
        configuration: LumiRunConfiguration,
        context: ConfigurationContext,
    ): Boolean {
        if (configuration.mode != LumiRunMode.BUILD_RUN) return false
        val entry = entryFor(context) ?: return false
        return configuration.filePath == entry
    }
}
