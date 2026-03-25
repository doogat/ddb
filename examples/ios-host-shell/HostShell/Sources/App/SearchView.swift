import SwiftUI

/// Cross-module full-text search across all doogat types.
struct SearchView: View {
    @EnvironmentObject private var appState: AppState
    @State private var query = ""
    @State private var results: [SearchResult] = []

    var body: some View {
        NavigationStack {
            List(results, id: \.id) { result in
                VStack(alignment: .leading) {
                    Text(result.title)
                        .font(.headline)
                    if !result.snippet.isEmpty {
                        Text(result.snippet)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            .navigationTitle("Search")
            .searchable(text: $query)
            .onChange(of: query) {
                performSearch()
            }
        }
    }

    private func performSearch() {
        guard !query.isEmpty else {
            results = []
            return
        }
        do {
            results = try appState.driver.search(query: query)
        } catch {
            results = []
        }
    }
}
