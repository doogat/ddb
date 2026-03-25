package com.doogat.hostshell.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Bookmark
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.doogat.hostshell.bookmarks.BookmarkListScreen
import com.doogat.hostshell.contacts.ContactListScreen

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val driver = (application as DDBApplication).driver

        setContent {
            MaterialTheme {
                HostShellScaffold(
                    onQueryBookmarks = { sql -> driver.executeSql(sql) },
                    onQueryContacts = { sql -> driver.executeSql(sql) },
                    onSearch = { query -> driver.search(query) }
                )
            }
        }
    }
}

@Composable
fun HostShellScaffold(
    onQueryBookmarks: (String) -> uniffi.ddb_core.SqlResultRecord,
    onQueryContacts: (String) -> uniffi.ddb_core.SqlResultRecord,
    onSearch: (String) -> List<uniffi.ddb_core.SearchResult>,
) {
    var selectedTab by remember { mutableIntStateOf(0) }

    Scaffold(
        bottomBar = {
            NavigationBar {
                NavigationBarItem(
                    selected = selectedTab == 0,
                    onClick = { selectedTab = 0 },
                    icon = { Icon(Icons.Default.Bookmark, contentDescription = "Bookmarks") },
                    label = { Text("Bookmarks") }
                )
                NavigationBarItem(
                    selected = selectedTab == 1,
                    onClick = { selectedTab = 1 },
                    icon = { Icon(Icons.Default.Person, contentDescription = "Contacts") },
                    label = { Text("Contacts") }
                )
                NavigationBarItem(
                    selected = selectedTab == 2,
                    onClick = { selectedTab = 2 },
                    icon = { Icon(Icons.Default.Search, contentDescription = "Search") },
                    label = { Text("Search") }
                )
            }
        }
    ) { padding ->
        when (selectedTab) {
            0 -> BookmarkListScreen(
                modifier = Modifier.padding(padding),
                onQuery = onQueryBookmarks
            )
            1 -> ContactListScreen(
                modifier = Modifier.padding(padding),
                onQuery = onQueryContacts
            )
            2 -> SearchScreen(
                modifier = Modifier.padding(padding),
                onSearch = onSearch
            )
        }
    }
}

@Composable
fun SearchScreen(
    modifier: Modifier = Modifier,
    onSearch: (String) -> List<uniffi.ddb_core.SearchResult>,
) {
    var query by remember { mutableStateOf("") }
    var results by remember { mutableStateOf(emptyList<uniffi.ddb_core.SearchResult>()) }

    Column(modifier = modifier) {
        OutlinedTextField(
            value = query,
            onValueChange = {
                query = it
                if (it.isNotBlank()) {
                    results = try { onSearch(it) } catch (_: Exception) { emptyList() }
                } else {
                    results = emptyList()
                }
            },
            label = { Text("Search all doogats") },
            modifier = Modifier.fillMaxWidth().padding(16.dp)
        )
        LazyColumn {
            items(results.size) { i ->
                val r = results[i]
                ListItem(
                    headlineContent = { Text(r.title) },
                    supportingContent = { Text(r.snippet) }
                )
            }
        }
    }
}
