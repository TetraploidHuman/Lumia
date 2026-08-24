package org.lumi.idea

object LumiTokenTypes {
    @JvmField val KEYWORD = LumiTokenType("KEYWORD")
    @JvmField val BUILTIN_TYPE = LumiTokenType("BUILTIN_TYPE")
    @JvmField val IDENTIFIER = LumiTokenType("IDENTIFIER")
    @JvmField val NUMBER = LumiTokenType("NUMBER")
    @JvmField val STRING = LumiTokenType("STRING")
    @JvmField val CHAR = LumiTokenType("CHAR")
    /** `$ident` or `${` start of string interpolation. */
    @JvmField val STRING_INTERP = LumiTokenType("STRING_INTERP")
    @JvmField val COMMENT = LumiTokenType("COMMENT")
    @JvmField val OPERATOR = LumiTokenType("OPERATOR")
    @JvmField val LBRACE = LumiTokenType("LBRACE")
    @JvmField val RBRACE = LumiTokenType("RBRACE")
    @JvmField val LPAREN = LumiTokenType("LPAREN")
    @JvmField val RPAREN = LumiTokenType("RPAREN")
    @JvmField val LBRACKET = LumiTokenType("LBRACKET")
    @JvmField val RBRACKET = LumiTokenType("RBRACKET")
}
