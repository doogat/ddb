# iOS Host-Shell Example

A SwiftUI host-shell app demonstrating two mini-app modules (Bookmarks and Contacts) sharing one embedded Doogat DB core.

## Prerequisites

1. Build the Doogat DB XCFramework:
   ```bash
   cd ../../
   dev/bin/build-xcframework
   ```

2. Generate Swift bindings:
   ```bash
   cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate \
     --library target/debug/libddb_core.dylib \
     --language swift --out-dir examples/ios-host-shell/HostShell/Sources/Shared
   ```

3. Open in Xcode:
   ```bash
   open HostShell.xcodeproj
   ```

4. Add the XCFramework to the project (drag `out/DdbCore.xcframework` into Xcode).

## Architecture

```
HostShellApp
├── AppState (owns DoogatDriver)
├── BookmarksModule
│   ├── bootstrap() — CREATE TABLE IF NOT EXISTS bookmark, category
│   └── BookmarkListView
├── ContactsModule
│   ├── bootstrap() — CREATE TABLE IF NOT EXISTS contact
│   └── ContactListView
└── SearchView (cross-module FTS5 search)
```

All modules share one `DoogatDriver` instance via `@EnvironmentObject`.

## Key patterns

- **Schema bootstrap**: each module uses `CREATE TABLE IF NOT EXISTS` for idempotent setup
- **Shared driver**: injected via SwiftUI environment
- **Cross-module search**: FTS5 search spans all doogat types
- **Tab navigation**: each module is a tab; search is a shared tab
