# Mobile Constraints

This page covers platform-specific constraints for mobile host-shell apps. Read [Building Apps](./building-apps.md#mobile-mini-apps) first for the host-shell architecture.

## Cross-Process Safety

When the host app and widgets/extensions run in separate processes, they share the same SQLite index file. Doogat DB uses WAL mode and a 5-second busy timeout to coordinate access safely.

### Rules

1. **Host app is the sole writer.** Only the host app process should create, update, or delete doogats. Widgets and extensions read from the index only.
2. **Widgets open read-only connections.** Use `SQLITE_OPEN_READONLY` (iOS) or `SQLiteDatabase.OPEN_READONLY` (Android) when opening `index.db` from a widget.
3. **WAL provides snapshot isolation.** A widget reading during a host app write sees a consistent pre-write snapshot — no torn reads, no locks.
4. **busy_timeout prevents SQLITE_BUSY.** If the host app is mid-checkpoint while a widget opens a connection, the widget retries for up to 5 seconds instead of failing immediately.
5. **Don't checkpoint from widgets.** Only the host app should checkpoint the WAL file. Widgets should leave `PRAGMA wal_checkpoint` alone.

### Exception: iOS Share Extension

A Share Extension may create a single doogat via an ephemeral `DoogatDriver`. This is safe because:
- Git commits are atomic (single file write + index update)
- The host app reindexes on next launch
- The extension closes the driver immediately after the operation

Do not hold `DoogatDriver` open in an extension beyond the single operation.

### What can go wrong

| Scenario | Outcome |
|----------|---------|
| Widget reads during host app write | Safe — WAL snapshot isolation |
| Widget reads during host app reindex | Safe — sees pre-reindex snapshot |
| Host app killed mid-write | Safe — SQLite WAL recovery on next open |
| Extension creates doogat while host app is running | Git lockfile serializes; one waits for the other |
| Two extensions write simultaneously | Git lockfile serializes; unlikely in practice |

## Background Execution

### iOS

iOS aggressively limits background execution:

| Mechanism | Duration | Guarantee |
|-----------|----------|-----------|
| Background task (UIApplication) | ~30 seconds | Best-effort, system can end early |
| BGAppRefreshTask | ~30 seconds | System-scheduled, not guaranteed |
| BGProcessingTask | Minutes | Only when plugged in, rare |
| Push notification trigger | ~30 seconds | Requires server-side push |

**Practical impact**: sync (`DoogatDriver` remote fetch/push) should run in the foreground or in a short background task. Do not rely on background execution for regular sync — it may not fire for hours or days.

**What not to attempt in background**:
- Full reindex (can take seconds at scale)
- Large sync operations (network + git merge + index update)
- CRDT conflict resolution (may require multiple git operations)

**What works in background**:
- Reading from SQLite index (instant, read-only)
- Scheduling a BGAppRefreshTask for next foreground sync
- Updating widget timelines from cached index data

### Android

Android background restrictions vary by OS version and manufacturer:

| Mechanism | Behavior |
|-----------|----------|
| WorkManager (periodic) | Minimum 15-minute interval, batched by system |
| WorkManager (one-time) | Runs when constraints met, may be deferred |
| Doze mode | Network and CPU suspended; only high-priority FCM breaks through |
| App Standby Buckets | Limits job frequency based on app usage |
| Manufacturer restrictions | Samsung, Xiaomi, etc. add extra kills |

**Practical impact**: same as iOS — sync in foreground, use WorkManager for opportunistic background sync, do not depend on timely execution.

## Widget Data Freshness

Widgets show data from the SQLite index. They cannot trigger sync or run the full Doogat DB core.

### iOS WidgetKit

- Widgets render from a **timeline** of entries
- The system decides when to refresh (budget-limited)
- Host app calls `WidgetCenter.shared.reloadAllTimelines()` after writes
- Widget reads `index.db` directly (read-only SQLite connection via App Group)
- Expect staleness: seconds (if app just wrote) to hours (if app hasn't run)

```swift
// In host app, after any write operation
WidgetCenter.shared.reloadAllTimelines()
```

```swift
// In widget TimelineProvider
func getTimeline(in context: Context, completion: @escaping (Timeline<Entry>) -> Void) {
    let dbPath = appGroupURL.appendingPathComponent(".ddb/index.db").path
    // Open read-only SQLite connection, query materialized tables
    let entries = queryRecentBookmarks(dbPath: dbPath)
    let timeline = Timeline(entries: entries, policy: .after(Date().addingTimeInterval(3600)))
    completion(timeline)
}
```

### Android AppWidgetProvider

- Widgets update via `onUpdate()` callback or explicit broadcast
- Host app sends `ACTION_APPWIDGET_UPDATE` after writes
- Widget reads `index.db` directly from app-private storage
- Use `RemoteViewsFactory` for collection widgets (ListView, GridView)

```kotlin
// In host app, after any write operation
val appWidgetManager = AppWidgetManager.getInstance(context)
val ids = appWidgetManager.getAppWidgetIds(ComponentName(context, BookmarkWidget::class.java))
appWidgetManager.notifyAppWidgetViewDataChanged(ids, R.id.bookmark_list)
```

## Extension Lifecycle

### iOS Share Extension

Share extensions let users send content to your app from other apps (Safari, Photos, etc.).

- Runs in a **separate process** from the host app
- Has access to App Group storage
- Can create a short-lived `DoogatDriver` to insert one doogat
- Must complete within the system time limit (~seconds)
- Host app reindexes on next launch to pick up the new doogat

```swift
// In ShareViewController
func didSelectPost() {
    let driver = try DoogatDriver(repoPath: appGroupRepoPath)
    let t = title.replacingOccurrences(of: "'", with: "''")
    let u = url.replacingOccurrences(of: "'", with: "''")
    try driver.executeSql("""
        INSERT INTO bookmark (title, url) VALUES ('\(t)', '\(u)')
    """)
    // Driver dropped, git commit is atomic
    extensionContext?.completeRequest(returningItems: nil)
}
```

**Warning**: do not hold the `DoogatDriver` open beyond the single operation. Extensions are killed without notice.

### iOS Action Extension

Action extensions (e.g., "Open in Doogat DB") are read-only — they display data but should not write. Open a read-only SQLite connection to the index.

### Android Share Target

```kotlin
// In ShareActivity
override fun onCreate(savedInstanceState: Bundle?) {
    val url = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return finish()
    val driver = DoogatDriver(repoPath = appGroupRepoPath)
    val safeUrl = url.replace("'", "''")
    driver.executeSql("INSERT INTO bookmark (title, url) VALUES ('Shared', '$safeUrl')")
    finish()
}
```

## Sync Strategy on Mobile

### Recommended approach

1. **Foreground sync**: trigger sync when the app becomes active (`onResume` / `scenePhase == .active`)
2. **User-initiated sync**: pull-to-refresh or explicit sync button
3. **Opportunistic background**: schedule via BGAppRefreshTask (iOS) or WorkManager (Android), but don't depend on it
4. **No always-on sync**: do not run a persistent background service or keep a network connection open

### Sync flow

```
App becomes active
  → check last sync timestamp
  → if stale (>N minutes), trigger sync
  → sync runs: fetch remote → merge → push → reindex
  → update widget timelines
  → save sync timestamp
```

### Conflict handling

Conflicts are resolved automatically by the CRDT resolver — no user intervention needed. On mobile, conflicts are more likely because sync is infrequent. The CRDT model handles this gracefully: all devices converge to the same state regardless of merge order.

## Performance Considerations

### Cold start budget

DoogatDriver initialization includes:
- Opening the git repo (~1-5ms)
- Opening/creating the SQLite index (~1-5ms)
- Schema bootstrap per module (~10-50ms per `listTypeSchemas()` + `CREATE TABLE` check)

Reference numbers from FFI performance tests (macOS, Apple Silicon):

| Repo size | Cold start | Create doogat | Search | Reindex |
|-----------|-----------|---------------|--------|---------|
| Empty | <50ms | ~20ms | <5ms | <10ms |
| 100 doogats | <100ms | ~25ms | <10ms | ~200ms |
| 1K doogats | <200ms | ~30ms | <15ms | ~2s |

Mobile devices are slower — expect 2-3x these numbers on older phones. A 1K-doogat repo should still initialize in under 500ms on modern devices.

**Recommendation**: initialize `DoogatDriver` on a background thread and show a loading indicator. Don't block the main/UI thread.

### Index reuse

The SQLite index persists across app launches. A full reindex is only needed when:
- The index file is deleted or corrupted
- The app detects the git HEAD has changed since last index update (new commits from sync)
- The user explicitly requests it

For normal app launches with no sync changes, the existing index is used as-is — no reindex cost.

#### Checking index freshness

To avoid unnecessary reindex after sync, compare the git HEAD commit before and after sync:

```swift
// Swift pseudocode
let headBefore = lastKnownHead  // stored in UserDefaults
driver.sync()                    // fetch + merge + push
let headAfter = currentGitHead() // read from .git/HEAD
if headBefore != headAfter {
    driver.reindex()
    UserDefaults.standard.set(headAfter, forKey: "lastIndexedHead")
}
```

### Lazy vs eager reindex

| Strategy | When to use |
|----------|-------------|
| **Eager** (reindex at launch) | Small repos (<500 doogats), sync always runs at launch |
| **Lazy** (reindex after sync only) | Larger repos, sync is user-triggered or background |

For most mobile apps, lazy reindex is better — users open the app far more often than they sync.

### Memory pressure

On iOS, respond to `didReceiveMemoryWarning` by dropping any cached data. The `DoogatDriver` itself holds minimal memory (just Mutex-wrapped handles). SQLite's page cache is the main consumer — it releases memory automatically under pressure.

On Android, consider closing the `DoogatDriver` in `onTrimMemory(TRIM_MEMORY_BACKGROUND)` and reopening on next use.
