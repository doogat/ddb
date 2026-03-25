package com.doogat.hostshell.contacts

import com.doogat.hostshell.DDBModule
import uniffi.ddb_core.DoogatDriver

object ContactsModule : DDBModule {
    override val tables = listOf("contact")

    override fun bootstrap(driver: DoogatDriver) {
        driver.executeSql("""
            CREATE TABLE IF NOT EXISTS contact (
                name TEXT NOT NULL,
                relationship TEXT,
                email TEXT
            )
        """.trimIndent())
    }
}
