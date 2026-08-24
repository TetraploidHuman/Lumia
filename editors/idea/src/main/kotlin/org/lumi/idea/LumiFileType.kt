package org.lumi.idea

import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

class LumiFileType private constructor() : LanguageFileType(LumiLanguage) {
    override fun getName(): String = "Lumi"
    override fun getDescription(): String = "Lumi source file"
    override fun getDefaultExtension(): String = "lm"
    override fun getIcon(): Icon = LumiIcons.FILE

    companion object {
        @JvmField
        val INSTANCE = LumiFileType()
    }
}
