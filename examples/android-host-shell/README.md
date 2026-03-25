# Android Host-Shell Example

A Jetpack Compose host-shell app demonstrating two mini-app modules (Bookmarks and Contacts) sharing one embedded Doogat DB core.

## Prerequisites

1. Build the Android AAR:
   ```bash
   cd ../../
   dev/bin/build-android
   ```

2. Generate Kotlin bindings:
   ```bash
   cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate \
     --library target/debug/libddb_core.dylib \
     --language kotlin --out-dir examples/android-host-shell/core-ddb/src/main/java
   ```

3. Open in Android Studio and sync Gradle.

## Architecture

```
HostShellApp (Application)
├── DoogatDriver (one instance, app-scoped)
├── :feature-bookmarks
│   ├── BookmarksModule.bootstrap()
│   └── BookmarkListScreen
├── :feature-contacts
│   ├── ContactsModule.bootstrap()
│   └── ContactListScreen
└── :core-ddb
    └── DDBModule interface + shared DoogatDriver access
```

## Module structure

```
android-host-shell/
├── app/                      Main app module
├── core-ddb/                 Shared DoogatDriver wrapper + module interface
├── feature-bookmarks/        Bookmarks mini-app
├── feature-contacts/         Contacts mini-app
├── build.gradle.kts          Root build file
└── settings.gradle.kts       Module declarations
```

## Key patterns

- **Schema bootstrap**: each module uses `CREATE TABLE IF NOT EXISTS` for idempotent setup
- **Shared driver**: provided by `DDBApplication`, accessed via `(application as DDBApplication).driver`
- **Bottom navigation**: each module is a destination
- **Cross-module search**: FTS5 search spans all doogat types
