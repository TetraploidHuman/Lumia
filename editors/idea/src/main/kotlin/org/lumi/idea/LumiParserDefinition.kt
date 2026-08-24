package org.lumi.idea

import com.intellij.lang.ASTNode
import com.intellij.lang.ParserDefinition
import com.intellij.lang.PsiBuilder
import com.intellij.lang.PsiParser
import com.intellij.lexer.Lexer
import com.intellij.openapi.project.Project
import com.intellij.psi.FileViewProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IElementType
import com.intellij.psi.tree.IFileElementType
import com.intellij.psi.tree.TokenSet

/**
 * Minimal lexer-backed PSI so quote/brace pairing and string highlighting
 * attach to a real Lumi [PsiFile] (not plain text).
 */
class LumiParserDefinition : ParserDefinition {
    override fun createLexer(project: Project?): Lexer = LumiLexer()

    override fun createParser(project: Project?): PsiParser =
        PsiParser { root: IElementType, builder: PsiBuilder ->
            val mark = builder.mark()
            while (!builder.eof()) {
                builder.advanceLexer()
            }
            mark.done(root)
            builder.treeBuilt
        }

    override fun getFileNodeType(): IFileElementType = FILE

    override fun getCommentTokens(): TokenSet = COMMENTS

    override fun getStringLiteralElements(): TokenSet = STRINGS

    override fun createElement(node: ASTNode): PsiElement =
        throw UnsupportedOperationException(node.elementType.toString())

    override fun createFile(viewProvider: FileViewProvider): PsiFile = LumiFile(viewProvider)

    companion object {
        val FILE = IFileElementType(LumiLanguage)
        val COMMENTS = TokenSet.create(LumiTokenTypes.COMMENT)
        val STRINGS = TokenSet.create(LumiTokenTypes.STRING, LumiTokenTypes.CHAR)
    }
}
