package org.lumia.idea

import com.intellij.codeInsight.editorActions.SimpleTokenSetQuoteHandler
import com.intellij.psi.tree.TokenSet

class LumiaQuoteHandler : SimpleTokenSetQuoteHandler(
    TokenSet.create(LumiaTokenTypes.STRING, LumiaTokenTypes.CHAR),
)
