package org.lumi.idea

import com.intellij.extapi.psi.PsiFileBase
import com.intellij.openapi.fileTypes.FileType
import com.intellij.psi.FileViewProvider

class LumiFile(viewProvider: FileViewProvider) : PsiFileBase(viewProvider, LumiLanguage) {
    override fun getFileType(): FileType = LumiFileType.INSTANCE

    override fun toString(): String = "Lumi File"
}
