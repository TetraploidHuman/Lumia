package org.lumia.idea

import com.intellij.openapi.fileTypes.LanguageFileType
import javax.swing.Icon

class LumiaFileType private constructor() : LanguageFileType(LumiaLanguage) {
    override fun getName(): String = "Lumia"
    override fun getDescription(): String = "Lumia source file"
    override fun getDefaultExtension(): String = "lm"
    override fun getIcon(): Icon = LumiaIcons.FILE

    companion object {
        @JvmField
        val INSTANCE = LumiaFileType()
    }
}
