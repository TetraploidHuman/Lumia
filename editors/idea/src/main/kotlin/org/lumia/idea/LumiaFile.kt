package org.lumia.idea

import com.intellij.extapi.psi.PsiFileBase
import com.intellij.openapi.fileTypes.FileType
import com.intellij.psi.FileViewProvider

class LumiaFile(viewProvider: FileViewProvider) : PsiFileBase(viewProvider, LumiaLanguage) {
    override fun getFileType(): FileType = LumiaFileType.INSTANCE

    override fun toString(): String = "Lumia File"
}
