package org.lumia.idea

import com.intellij.codeInsight.template.TemplateContextType
import com.intellij.psi.PsiFile

class LumiaContextType : TemplateContextType("Lumia") {
    override fun isInContext(file: PsiFile, offset: Int): Boolean =
        file.language.isKindOf(LumiaLanguage)
}
