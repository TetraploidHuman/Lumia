package org.lumi.idea

import com.intellij.lexer.Lexer
import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.psi.tree.IElementType

class LumiSyntaxHighlighter : SyntaxHighlighterBase() {
    override fun getHighlightingLexer(): Lexer = LumiLexer()

    override fun getTokenHighlights(tokenType: IElementType?): Array<TextAttributesKey> =
        when (tokenType) {
            LumiTokenTypes.KEYWORD -> KEYWORD_KEYS
            LumiTokenTypes.BUILTIN_TYPE -> TYPE_KEYS
            LumiTokenTypes.STRING, LumiTokenTypes.CHAR -> STRING_KEYS
            LumiTokenTypes.STRING_INTERP -> INTERP_KEYS
            LumiTokenTypes.NUMBER -> NUMBER_KEYS
            LumiTokenTypes.COMMENT -> COMMENT_KEYS
            LumiTokenTypes.OPERATOR -> OPERATOR_KEYS
            LumiTokenTypes.IDENTIFIER -> IDENT_KEYS
            LumiTokenTypes.LBRACE, LumiTokenTypes.RBRACE,
            LumiTokenTypes.LPAREN, LumiTokenTypes.RPAREN,
            LumiTokenTypes.LBRACKET, LumiTokenTypes.RBRACKET,
            -> BRACES_KEYS
            else -> emptyArray()
        }

    companion object {
        private val KEYWORD_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_KEYWORD", DefaultLanguageHighlighterColors.KEYWORD),
        )
        private val TYPE_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_TYPE", DefaultLanguageHighlighterColors.CLASS_NAME),
        )
        private val STRING_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_STRING", DefaultLanguageHighlighterColors.STRING),
        )
        private val INTERP_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey(
                "LUMI_STRING_INTERP",
                DefaultLanguageHighlighterColors.VALID_STRING_ESCAPE,
            ),
        )
        private val NUMBER_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_NUMBER", DefaultLanguageHighlighterColors.NUMBER),
        )
        private val COMMENT_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_COMMENT", DefaultLanguageHighlighterColors.LINE_COMMENT),
        )
        private val OPERATOR_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_OP", DefaultLanguageHighlighterColors.OPERATION_SIGN),
        )
        private val IDENT_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_IDENT", DefaultLanguageHighlighterColors.IDENTIFIER),
        )
        private val BRACES_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMI_BRACES", DefaultLanguageHighlighterColors.BRACES),
        )
    }
}
