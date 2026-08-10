package org.lumia.idea

import com.intellij.psi.codeStyle.CodeStyleSettingsCustomizable
import com.intellij.psi.codeStyle.CommonCodeStyleSettings
import com.intellij.psi.codeStyle.LanguageCodeStyleSettingsProvider

class LumiaLanguageCodeStyleSettingsProvider : LanguageCodeStyleSettingsProvider() {
    override fun getLanguage() = LumiaLanguage

    override fun customizeSettings(
        consumer: CodeStyleSettingsCustomizable,
        settingsType: SettingsType,
    ) {
        if (settingsType == SettingsType.INDENT_SETTINGS) {
            consumer.showStandardOptions(
                "INDENT_SIZE",
                "CONTINUATION_INDENT_SIZE",
                "TAB_SIZE",
                "USE_TAB_CHARACTER",
            )
        }
    }

    override fun customizeDefaults(
        commonSettings: CommonCodeStyleSettings,
        indentOptions: CommonCodeStyleSettings.IndentOptions,
    ) {
        indentOptions.INDENT_SIZE = 4
        indentOptions.CONTINUATION_INDENT_SIZE = 4
        indentOptions.TAB_SIZE = 4
        indentOptions.USE_TAB_CHARACTER = false
    }

    override fun getCodeSample(settingsType: SettingsType): String =
        """
        |module Demo
        |
        |import std.io.{println}
        |
        |val main = {
        |    println(42)
        |}
        """.trimMargin()
}
