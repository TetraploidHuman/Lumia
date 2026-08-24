package org.lumi.idea

import com.intellij.lang.BracePair
import com.intellij.lang.PairedBraceMatcher
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IElementType

class LumiBraceMatcher : PairedBraceMatcher {
    override fun getPairs(): Array<BracePair> = PAIRS

    override fun isPairedBracesAllowedBeforeType(
        lbraceType: IElementType,
        contextType: IElementType?,
    ): Boolean = true

    override fun getCodeConstructStart(file: PsiFile, openingBraceOffset: Int): Int =
        openingBraceOffset

    companion object {
        private val PAIRS = arrayOf(
            BracePair(LumiTokenTypes.LBRACE, LumiTokenTypes.RBRACE, true),
            BracePair(LumiTokenTypes.LPAREN, LumiTokenTypes.RPAREN, false),
            BracePair(LumiTokenTypes.LBRACKET, LumiTokenTypes.RBRACKET, false),
        )
    }
}
