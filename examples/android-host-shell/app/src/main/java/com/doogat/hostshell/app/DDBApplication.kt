package com.doogat.hostshell.app

import android.app.Application
import com.doogat.hostshell.bookmarks.BookmarksModule
import com.doogat.hostshell.contacts.ContactsModule
import uniffi.ddb_core.DoogatDriver
import java.io.File

class DDBApplication : Application() {
    lateinit var driver: DoogatDriver
        private set

    override fun onCreate() {
        super.onCreate()

        val repoPath = File(filesDir, "ddb").path
        driver = if (File(repoPath, ".git").exists()) {
            DoogatDriver(repoPath)
        } else {
            DoogatDriver.createRepo(repoPath).also {
                it.registerNode("android-host-shell")
            }
        }

        // Bootstrap modules in dependency order
        BookmarksModule.bootstrap(driver)
        ContactsModule.bootstrap(driver)
    }
}
