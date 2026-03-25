package com.doogat.hostshell

import uniffi.ddb_core.DoogatDriver

/**
 * Interface for host-shell feature modules.
 * Each module declares its tables and bootstraps its schema.
 */
interface DDBModule {
    val tables: List<String>
    fun bootstrap(driver: DoogatDriver)
}

fun columnValue(row: List<String>, columns: List<String>, name: String): String {
    val idx = columns.indexOf(name)
    return if (idx in row.indices) row[idx] else ""
}
