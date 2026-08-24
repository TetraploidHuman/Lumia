package org.lumi.idea

import com.intellij.codeInsight.editorActions.SimpleTokenSetQuoteHandler
import com.intellij.psi.tree.TokenSet

class LumiQuoteHandler : SimpleTokenSetQuoteHandler(
    TokenSet.create(LumiTokenTypes.STRING, LumiTokenTypes.CHAR),
)
