package org.lumia.idea

import com.intellij.lang.BracePair
import com.intellij.lang.PairedBraceMatcher
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IElementType

class LumiaBraceMatcher : PairedBraceMatcher {
    override fun getPairs(): Array<BracePair> = PAIRS

    override fun isPairedBracesAllowedBeforeType(
        lbraceType: IElementType,
        contextType: IElementType?,
    ): Boolean = true

    override fun getCodeConstructStart(file: PsiFile, openingBraceOffset: Int): Int =
        openingBraceOffset

    companion object {
        private val PAIRS = arrayOf(
            BracePair(LumiaTokenTypes.LBRACE, LumiaTokenTypes.RBRACE, true),
            BracePair(LumiaTokenTypes.LPAREN, LumiaTokenTypes.RPAREN, false),
            BracePair(LumiaTokenTypes.LBRACKET, LumiaTokenTypes.RBRACKET, false),
        )
    }
}
