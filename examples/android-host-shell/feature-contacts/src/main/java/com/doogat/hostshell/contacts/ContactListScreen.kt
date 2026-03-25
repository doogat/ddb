package com.doogat.hostshell.contacts

import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import com.doogat.hostshell.columnValue
import uniffi.ddb_core.SqlResultRecord

@Composable
fun ContactListScreen(
    modifier: Modifier = Modifier,
    onQuery: (String) -> SqlResultRecord,
) {
    var rows by remember { mutableStateOf(emptyList<List<String>>()) }
    var columns by remember { mutableStateOf(emptyList<String>()) }

    LaunchedEffect(Unit) {
        val result = onQuery("SELECT id, name, relationship, email FROM contact")
        columns = result.columns
        rows = result.rows
    }

    LazyColumn(modifier = modifier) {
        items(rows.size) { i ->
            val row = rows[i]
            val name = columnValue(row, columns, "name")
            val email = columnValue(row, columns, "email")
            val relationship = columnValue(row, columns, "relationship")
            ListItem(
                headlineContent = { Text(name) },
                supportingContent = {
                    Text(listOfNotNull(
                        email.ifBlank { null },
                        relationship.ifBlank { null }
                    ).joinToString(" · "))
                }
            )
            if (i < rows.size - 1) HorizontalDivider()
        }
    }
}
