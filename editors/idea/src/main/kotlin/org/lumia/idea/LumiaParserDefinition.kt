package org.lumia.idea

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
 * attach to a real Lumia [PsiFile] (not plain text).
 */
class LumiaParserDefinition : ParserDefinition {
    override fun createLexer(project: Project?): Lexer = LumiaLexer()

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

    override fun createFile(viewProvider: FileViewProvider): PsiFile = LumiaFile(viewProvider)

    companion object {
        val FILE = IFileElementType(LumiaLanguage)
        val COMMENTS = TokenSet.create(LumiaTokenTypes.COMMENT)
        val STRINGS = TokenSet.create(LumiaTokenTypes.STRING, LumiaTokenTypes.CHAR)
    }
}
