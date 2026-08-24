package org.lumi.idea

import com.intellij.lang.Language

object LumiLanguage : Language("Lumi") {
    private fun readResolve(): Any = LumiLanguage
}
