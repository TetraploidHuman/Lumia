package org.lumi.idea

import com.intellij.codeInsight.editorActions.enter.EnterBetweenBracesDelegate

/** Inserts a blank indented line when Enter is pressed between `{|}` / `(|)` / `[|]`. */
class LumiEnterBetweenBracesHandler : EnterBetweenBracesDelegate()
