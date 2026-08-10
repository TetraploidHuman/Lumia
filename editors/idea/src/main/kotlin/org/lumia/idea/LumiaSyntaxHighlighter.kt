package org.lumia.idea

import com.intellij.lexer.Lexer
import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.psi.tree.IElementType

class LumiaSyntaxHighlighter : SyntaxHighlighterBase() {
    override fun getHighlightingLexer(): Lexer = LumiaLexer()

    override fun getTokenHighlights(tokenType: IElementType?): Array<TextAttributesKey> =
        when (tokenType) {
            LumiaTokenTypes.KEYWORD -> KEYWORD_KEYS
            LumiaTokenTypes.BUILTIN_TYPE -> TYPE_KEYS
            LumiaTokenTypes.STRING, LumiaTokenTypes.CHAR -> STRING_KEYS
            LumiaTokenTypes.STRING_INTERP -> INTERP_KEYS
            LumiaTokenTypes.NUMBER -> NUMBER_KEYS
            LumiaTokenTypes.COMMENT -> COMMENT_KEYS
            LumiaTokenTypes.OPERATOR -> OPERATOR_KEYS
            LumiaTokenTypes.IDENTIFIER -> IDENT_KEYS
            LumiaTokenTypes.LBRACE, LumiaTokenTypes.RBRACE,
            LumiaTokenTypes.LPAREN, LumiaTokenTypes.RPAREN,
            LumiaTokenTypes.LBRACKET, LumiaTokenTypes.RBRACKET,
            -> BRACES_KEYS
            else -> emptyArray()
        }

    companion object {
        private val KEYWORD_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_KEYWORD", DefaultLanguageHighlighterColors.KEYWORD),
        )
        private val TYPE_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_TYPE", DefaultLanguageHighlighterColors.CLASS_NAME),
        )
        private val STRING_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_STRING", DefaultLanguageHighlighterColors.STRING),
        )
        private val INTERP_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey(
                "LUMIA_STRING_INTERP",
                DefaultLanguageHighlighterColors.VALID_STRING_ESCAPE,
            ),
        )
        private val NUMBER_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_NUMBER", DefaultLanguageHighlighterColors.NUMBER),
        )
        private val COMMENT_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_COMMENT", DefaultLanguageHighlighterColors.LINE_COMMENT),
        )
        private val OPERATOR_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_OP", DefaultLanguageHighlighterColors.OPERATION_SIGN),
        )
        private val IDENT_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_IDENT", DefaultLanguageHighlighterColors.IDENTIFIER),
        )
        private val BRACES_KEYS = arrayOf(
            TextAttributesKey.createTextAttributesKey("LUMIA_BRACES", DefaultLanguageHighlighterColors.BRACES),
        )
    }
}
