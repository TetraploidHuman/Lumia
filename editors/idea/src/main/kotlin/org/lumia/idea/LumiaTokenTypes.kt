package org.lumia.idea

object LumiaTokenTypes {
    @JvmField val KEYWORD = LumiaTokenType("KEYWORD")
    @JvmField val BUILTIN_TYPE = LumiaTokenType("BUILTIN_TYPE")
    @JvmField val IDENTIFIER = LumiaTokenType("IDENTIFIER")
    @JvmField val NUMBER = LumiaTokenType("NUMBER")
    @JvmField val STRING = LumiaTokenType("STRING")
    @JvmField val CHAR = LumiaTokenType("CHAR")
    /** `$ident` or `${` start of string interpolation. */
    @JvmField val STRING_INTERP = LumiaTokenType("STRING_INTERP")
    @JvmField val COMMENT = LumiaTokenType("COMMENT")
    @JvmField val OPERATOR = LumiaTokenType("OPERATOR")
    @JvmField val LBRACE = LumiaTokenType("LBRACE")
    @JvmField val RBRACE = LumiaTokenType("RBRACE")
    @JvmField val LPAREN = LumiaTokenType("LPAREN")
    @JvmField val RPAREN = LumiaTokenType("RPAREN")
    @JvmField val LBRACKET = LumiaTokenType("LBRACKET")
    @JvmField val RBRACKET = LumiaTokenType("RBRACKET")
}
