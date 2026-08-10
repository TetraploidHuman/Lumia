package org.lumia.idea

import com.intellij.lang.Language

object LumiaLanguage : Language("Lumia") {
    private fun readResolve(): Any = LumiaLanguage
}
