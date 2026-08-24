package org.lumi.idea

import com.intellij.codeInsight.template.TemplateContextType
import com.intellij.psi.PsiFile

class LumiContextType : TemplateContextType("Lumi") {
    override fun isInContext(file: PsiFile, offset: Int): Boolean =
        file.language.isKindOf(LumiLanguage)
}
