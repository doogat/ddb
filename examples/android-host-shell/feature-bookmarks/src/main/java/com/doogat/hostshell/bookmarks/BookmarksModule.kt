package com.doogat.hostshell.bookmarks

import com.doogat.hostshell.DDBModule
import uniffi.ddb_core.DoogatDriver

object BookmarksModule : DDBModule {
    override val tables = listOf("category", "bookmark")

    override fun bootstrap(driver: DoogatDriver) {
        driver.executeSql("CREATE TABLE IF NOT EXISTS category (name TEXT NOT NULL)")
        driver.executeSql("""
            CREATE TABLE IF NOT EXISTS bookmark (
                title TEXT NOT NULL,
                url TEXT NOT NULL,
                category TEXT REFERENCES category(id)
            )
        """.trimIndent())
    }
}
