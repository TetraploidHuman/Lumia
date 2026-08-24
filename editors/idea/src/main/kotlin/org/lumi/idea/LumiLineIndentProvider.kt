package org.lumi.idea

import com.intellij.lang.Language
import com.intellij.openapi.editor.Editor
import com.intellij.openapi.project.Project
import com.intellij.psi.codeStyle.CodeStyleSettingsManager
import com.intellij.psi.codeStyle.lineIndent.LineIndentProvider

/**
 * Indent the new line after Enter based on the previous non-blank line:
 * +1 level if it ends with an unmatched opener (`{`/`(`/`[`);
 * align with a closing `}`/`)`/`]` at the start of the new line.
 */
class LumiLineIndentProvider : LineIndentProvider {
    override fun isSuitableFor(language: Language?): Boolean =
        language != null && language.isKindOf(LumiLanguage)

    override fun getLineIndent(project: Project, editor: Editor, language: Language?, offset: Int): String? {
        if (!isSuitableFor(language)) return null
        val doc = editor.document
        if (offset < 0 || offset > doc.textLength) return null

        val indentOpts = CodeStyleSettingsManager.getInstance(project)
            .mainProjectCodeStyle
            ?.getCommonSettings(LumiLanguage)
            ?.indentOptions
            ?: return "    "
        val indentSize = indentOpts.INDENT_SIZE.coerceAtLeast(1)
        val useTabs = indentOpts.USE_TAB_CHARACTER
        val tabSize = indentOpts.TAB_SIZE.coerceAtLeast(1)

        val text = doc.charsSequence
        val line = doc.getLineNumber(offset.coerceAtMost(doc.textLength))
        if (line == 0) return ""

        // Indent of the previous non-blank line.
        var prevLine = line - 1
        while (prevLine >= 0) {
            val a = doc.getLineStartOffset(prevLine)
            val b = doc.getLineEndOffset(prevLine)
            if (text.subSequence(a, b).isNotBlank()) break
            prevLine--
        }
        if (prevLine < 0) return ""

        val prevStart = doc.getLineStartOffset(prevLine)
        val prevEnd = doc.getLineEndOffset(prevLine)
        val prev = text.subSequence(prevStart, prevEnd).toString()
        val baseCols = leadingColumns(prev, tabSize)

        val trimmedEnd = prev.trimEnd()
        val bump =
            when {
                trimmedEnd.endsWith('{') || trimmedEnd.endsWith('(') || trimmedEnd.endsWith('[') -> indentSize
                else -> 0
            }

        // If the caret line already starts with a closer, pull indent back one level.
        val curStart = doc.getLineStartOffset(line)
        val curEnd = doc.getLineEndOffset(line).coerceAtLeast(curStart)
        val curTrim = text.subSequence(curStart, curEnd).toString().trimStart()
        val pull =
            when {
                curTrim.startsWith('}') || curTrim.startsWith(')') || curTrim.startsWith(']') -> indentSize
                else -> 0
            }

        val cols = (baseCols + bump - pull).coerceAtLeast(0)
        return formatIndent(cols, useTabs, tabSize)
    }

    private fun leadingColumns(line: String, tabSize: Int): Int {
        var cols = 0
        for (c in line) {
            when (c) {
                ' ' -> cols++
                '\t' -> cols += tabSize - (cols % tabSize)
                else -> break
            }
        }
        return cols
    }

    private fun formatIndent(cols: Int, useTabs: Boolean, tabSize: Int): String {
        if (!useTabs) return " ".repeat(cols)
        val tabs = cols / tabSize
        val spaces = cols % tabSize
        return "\t".repeat(tabs) + " ".repeat(spaces)
    }
}
