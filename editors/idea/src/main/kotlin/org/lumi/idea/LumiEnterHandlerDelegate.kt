package org.lumi.idea

import com.intellij.application.options.CodeStyle
import com.intellij.codeInsight.editorActions.enter.BaseIndentEnterHandler
import com.intellij.codeInsight.editorActions.enter.EnterHandlerDelegate
import com.intellij.openapi.actionSystem.DataContext
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.editor.actionSystem.EditorActionHandler
import com.intellij.openapi.util.Ref
import com.intellij.psi.PsiFile
import com.intellij.psi.TokenType
import com.intellij.psi.tree.TokenSet
import com.intellij.util.text.CharArrayUtil

/**
 * Enter inside `{|}` expands to a three-line block in one edit:
 * ```
 * {
 *     |
 * }
 * ```
 * Other positions fall back to [BaseIndentEnterHandler] with caret correction.
 */
class LumiEnterHandlerDelegate : BaseIndentEnterHandler(
    LumiLanguage,
    TokenSet.create(LumiTokenTypes.LBRACE, LumiTokenTypes.LPAREN, LumiTokenTypes.LBRACKET),
    LumiTokenTypes.COMMENT,
    "//",
    TokenSet.create(TokenType.WHITE_SPACE),
    false,
) {
    override fun preprocessEnter(
        file: PsiFile,
        editor: Editor,
        caretOffset: Ref<Int>,
        caretAdvance: Ref<Int>,
        dataContext: DataContext,
        originalHandler: EditorActionHandler?,
    ): EnterHandlerDelegate.Result {
        if (file.language != LumiLanguage) {
            return EnterHandlerDelegate.Result.Continue
        }
        tryEnterBetweenBraces(file, editor)?.let { return it }

        val offsetBefore = editor.caretModel.offset
        val lengthBefore = editor.document.textLength
        val result = super.preprocessEnter(
            file,
            editor,
            caretOffset,
            caretAdvance,
            dataContext,
            originalHandler,
        )
        if (result == EnterHandlerDelegate.Result.Stop &&
            editor.document.textLength > lengthBefore
        ) {
            moveCaretAfterInsertedIndent(editor, offsetBefore)
        }
        return result
    }

    /** `{|}` (whitespace-only inside) → middle line indented, `}` on its own line. */
    private fun tryEnterBetweenBraces(
        file: PsiFile,
        editor: Editor,
    ): EnterHandlerDelegate.Result? {
        val doc = editor.document
        val text = doc.charsSequence
        val caret = editor.caretModel.offset
        if (caret <= 0 || caret > text.length) return null

        val lBrace = CharArrayUtil.shiftBackward(text, caret - 1, " \t\n\r")
        if (lBrace < 0 || text[lBrace] != '{') return null

        val rBrace = CharArrayUtil.shiftForward(text, caret, " \t\n\r")
        if (rBrace >= text.length || text[rBrace] != '}') return null

        for (i in (lBrace + 1) until rBrace) {
            if (!text[i].isWhitespace()) return null
        }

        val line = doc.getLineNumber(lBrace)
        val lineStart = doc.getLineStartOffset(line)
        val lineIndentEnd = CharArrayUtil.shiftForward(text, lineStart, " \t")
        val lineIndent = text.subSequence(lineStart, lineIndentEnd).toString()
        val contentIndent = lineIndent + indentUnit(file)

        val insert = "\n$contentIndent\n$lineIndent"
        doc.replaceString(lBrace + 1, rBrace, insert)
        editor.caretModel.moveToOffset(lBrace + 1 + 1 + contentIndent.length)
        return EnterHandlerDelegate.Result.Stop
    }

    private fun indentUnit(file: PsiFile): String {
        val opts = CodeStyle.getSettings(file).getIndentOptions(file.fileType)
        return if (opts.USE_TAB_CHARACTER) "\t" else " ".repeat(opts.INDENT_SIZE.coerceAtLeast(1))
    }

    private fun moveCaretAfterInsertedIndent(editor: Editor, insertOffset: Int) {
        val doc = editor.document
        if (insertOffset < 0 || insertOffset >= doc.textLength) return
        var pos = insertOffset
        if (doc.charsSequence[pos] == '\n') pos++
        while (pos < doc.textLength) {
            when (doc.charsSequence[pos]) {
                ' ', '\t' -> pos++
                else -> break
            }
        }
        editor.caretModel.moveToOffset(pos)
    }
}
