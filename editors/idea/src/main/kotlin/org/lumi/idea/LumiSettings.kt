package org.lumi.idea

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.PersistentStateComponent
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.util.xmlb.XmlSerializerUtil

@State(name = "LumiSettings", storages = [Storage("LumiSettings.xml")])
class LumiSettings : PersistentStateComponent<LumiSettings> {
    /** Path to the `lumi` binary. */
    var lspPath: String = "lumi"

    override fun getState(): LumiSettings = this

    override fun loadState(state: LumiSettings) {
        XmlSerializerUtil.copyBean(state, this)
    }

    companion object {
        fun getInstance(): LumiSettings =
            ApplicationManager.getApplication().getService(LumiSettings::class.java)
    }
}
