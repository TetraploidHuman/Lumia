package org.lumia.idea

import com.intellij.lexer.LexerBase
import com.intellij.psi.TokenType
import com.intellij.psi.tree.IElementType

/**
 * Hand-written lexer for Lumia highlighting.
 *
 * String interpolation uses `$ident` / `${…}` (not bare `{…}`).
 */
class LumiaLexer : LexerBase() {
    private var buffer: CharSequence = ""
    private var end = 0
    private var tokenStart = 0
    private var tokenEnd = 0
    private var tokenType: IElementType? = null

    /** 0 = normal, 1 = inside `"…"`, 2+ = inside `${…}` with brace depth (state-1). */
    private var state = 0

    override fun start(buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer
        this.end = endOffset
        this.tokenStart = startOffset
        this.tokenEnd = startOffset
        this.state = initialState
        advance()
    }

    override fun getState(): Int = state
    override fun getTokenType(): IElementType? = tokenType
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = end

    override fun advance() {
        tokenStart = tokenEnd
        if (tokenStart >= end) {
            tokenType = null
            return
        }
        when {
            state == 0 -> advanceNormal()
            state == 1 -> advanceInString()
            else -> advanceInInterp()
        }
    }

    private fun advanceNormal() {
        val c = buffer[tokenStart]
        when {
            c.isWhitespace() -> {
                var i = tokenStart + 1
                while (i < end && buffer[i].isWhitespace()) i++
                tokenEnd = i
                tokenType = TokenType.WHITE_SPACE
            }
            c == '/' && tokenStart + 1 < end && buffer[tokenStart + 1] == '/' -> {
                var i = tokenStart + 2
                while (i < end && buffer[i] != '\n') i++
                tokenEnd = i
                tokenType = LumiaTokenTypes.COMMENT
            }
            c == '/' && tokenStart + 1 < end && buffer[tokenStart + 1] == '*' -> {
                var i = tokenStart + 2
                while (i + 1 < end && !(buffer[i] == '*' && buffer[i + 1] == '/')) i++
                tokenEnd = if (i + 1 < end) i + 2 else end
                tokenType = LumiaTokenTypes.COMMENT
            }
            c == '"' -> {
                tokenEnd = tokenStart + 1
                tokenType = LumiaTokenTypes.STRING
                state = 1
            }
            c == '\'' -> advanceCharLiteral()
            else -> advanceCodeAtom()
        }
    }

    private fun advanceCharLiteral() {
        var i = tokenStart + 1
        if (i < end && buffer[i] == '\\' && i + 1 < end) {
            i += 2
        } else if (i < end) {
            i++
        }
        if (i < end && buffer[i] == '\'') i++
        tokenEnd = i
        tokenType = LumiaTokenTypes.CHAR
    }

    private fun advanceInString() {
        val c = buffer[tokenStart]
        when {
            c == '"' -> {
                tokenEnd = tokenStart + 1
                tokenType = LumiaTokenTypes.STRING
                state = 0
            }
            c == '\\' && tokenStart + 1 < end -> {
                tokenEnd = tokenStart + 2
                tokenType = LumiaTokenTypes.STRING
            }
            c == '$' && tokenStart + 1 < end && buffer[tokenStart + 1] == '{' -> {
                tokenEnd = tokenStart + 2
                tokenType = LumiaTokenTypes.STRING_INTERP
                state = 2
            }
            c == '$' && tokenStart + 1 < end &&
                (buffer[tokenStart + 1].isLetter() || buffer[tokenStart + 1] == '_') -> {
                var i = tokenStart + 2
                while (i < end && (buffer[i].isLetterOrDigit() || buffer[i] == '_')) i++
                tokenEnd = i
                tokenType = LumiaTokenTypes.STRING_INTERP
            }
            else -> {
                var i = tokenStart + 1
                while (i < end) {
                    val ch = buffer[i]
                    if (ch == '"' || ch == '$' || ch == '\\') break
                    i++
                }
                tokenEnd = i
                tokenType = LumiaTokenTypes.STRING
            }
        }
    }

    private fun advanceInInterp() {
        val depth = state - 1
        val c = buffer[tokenStart]
        when {
            c == '{' -> {
                tokenEnd = tokenStart + 1
                tokenType = LumiaTokenTypes.LBRACE
                state += 1
            }
            c == '}' -> {
                tokenEnd = tokenStart + 1
                if (depth <= 1) {
                    tokenType = LumiaTokenTypes.STRING_INTERP
                    state = 1
                } else {
                    tokenType = LumiaTokenTypes.RBRACE
                    state -= 1
                }
            }
            c.isWhitespace() -> {
                var i = tokenStart + 1
                while (i < end && buffer[i].isWhitespace()) i++
                tokenEnd = i
                tokenType = TokenType.WHITE_SPACE
            }
            c == '"' -> {
                var i = tokenStart + 1
                while (i < end) {
                    val ch = buffer[i]
                    if (ch == '\\' && i + 1 < end) {
                        i += 2
                        continue
                    }
                    if (ch == '"') {
                        i++
                        break
                    }
                    i++
                }
                tokenEnd = i
                tokenType = LumiaTokenTypes.STRING
            }
            else -> advanceCodeAtom()
        }
    }

    private fun advanceCodeAtom() {
        val c = buffer[tokenStart]
        when {
            c.isDigit() -> {
                var i = tokenStart + 1
                while (i < end && (buffer[i].isDigit() || buffer[i] == '_' || buffer[i] == '.')) i++
                tokenEnd = i
                tokenType = LumiaTokenTypes.NUMBER
            }
            c.isLetter() || c == '_' -> {
                var i = tokenStart + 1
                while (i < end && (buffer[i].isLetterOrDigit() || buffer[i] == '_')) i++
                tokenEnd = i
                val text = buffer.subSequence(tokenStart, tokenEnd).toString()
                tokenType = when {
                    KEYWORDS.contains(text) -> LumiaTokenTypes.KEYWORD
                    BUILTIN_TYPES.contains(text) -> LumiaTokenTypes.BUILTIN_TYPE
                    else -> LumiaTokenTypes.IDENTIFIER
                }
            }
            else -> {
                // Multi-char operators
                val two = if (tokenStart + 1 < end) {
                    buffer.subSequence(tokenStart, tokenStart + 2).toString()
                } else ""
                val three = if (tokenStart + 2 < end) {
                    buffer.subSequence(tokenStart, tokenStart + 3).toString()
                } else ""
                when {
                    three == "..=" -> {
                        tokenEnd = tokenStart + 3
                        tokenType = LumiaTokenTypes.OPERATOR
                    }
                    two in MULTI_OPS -> {
                        tokenEnd = tokenStart + 2
                        tokenType = LumiaTokenTypes.OPERATOR
                    }
                    else -> {
                        tokenEnd = tokenStart + 1
                        tokenType = when (c) {
                            '{' -> LumiaTokenTypes.LBRACE
                            '}' -> LumiaTokenTypes.RBRACE
                            '(' -> LumiaTokenTypes.LPAREN
                            ')' -> LumiaTokenTypes.RPAREN
                            '[' -> LumiaTokenTypes.LBRACKET
                            ']' -> LumiaTokenTypes.RBRACKET
                            else -> LumiaTokenTypes.OPERATOR
                        }
                    }
                }
            }
        }
    }

    companion object {
        private val KEYWORDS = setOf(
            // Must stay aligned with `lumia_syntax::TokenKind::KEYWORDS` (+ SURFACE_SOFT).
            "module", "import", "val", "var", "type", "if", "else", "match", "for", "in",
            "break", "continue", "return", "alt", "and", "or", "not", "true", "false",
            "priv", "as", "trait", "instance", "requires", "with", "effect", "scope", "spawn",
            "foreign",
            // Surface soft (not lexer keywords): foreign decls.
            "pure", "fn",
        )
        private val BUILTIN_TYPES = setOf(
            "Int", "Float", "Bool", "String", "Char", "Unit", "List", "Map", "Set",
            "Option", "Result",
        )
        private val MULTI_OPS = setOf(
            "->", "=>", ">>", "..", "::", "==", "!=", "<=", ">=",
        )
    }
}
