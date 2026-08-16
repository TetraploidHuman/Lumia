package org.lumia.idea

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.util.xmlb.XmlSerializerUtil

@State(name = "LumiaSettings", storages = [Storage("LumiaSettings.xml")])
class LumiaSettings : PersistentStateComponent<LumiaSettings> {
    /** Path to the `lumia` binary. */
    var lspPath: String = "lumia"

    /** Mirror CLI auto-parallel (false ≈ `--no-parallel`). */
    var autoParallel: Boolean = true

    override fun getState(): LumiaSettings = this

    override fun loadState(state: LumiaSettings) {
        XmlSerializerUtil.copyBean(state, this)
    }

    companion object {
        fun getInstance(): LumiaSettings =
            ApplicationManager.getApplication().getService(LumiaSettings::class.java)
    }
}
