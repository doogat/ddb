# Design: Decouple sync/compact from the write actor; bound remote ops

## Architecture fit

Server layer (`ddb-server/src/actor/`, `maintenance.rs`, `config.rs`) and core git layer
(`ddb-core/src/git_ops/{mod.rs,remote.rs}`, `error.rs`, `app_contract/error.rs`). No change
to `DoogatService` itself, the CLI, or FFI — both already use their own in-process
`DoogatService` and never touch the server's actor.

Phase 0 Task 2 ("maintenance subprocesses run after guard drop") is **already shipped**:
`GitRepo::with_write_lock` (`ddb-core/src/git_ops/mod.rs:337-364`) already defers
`check_write_threshold` and `write_commit_graph` until after the OS lock guard drops
(the FT-5 fix, landed before this PRD), with regression tests
`every_nested_commit_counts_toward_maintenance_after_release` and
`deferred_checks_cross_the_write_threshold_like_inline_ones` already pinning it. This
design covers only the two capabilities still open: the remote timeout and the
maintenance-worker split; the NoSQL decision is already recorded (PRD body, "VERIFIED")
and needs only the server.md doc note + the Phase-2 bounded-latency test.

## Module placement

- `ddb-core/src/git_ops/remote.rs` (edit) — deadline-aware `fetch`/`push`; one shared
  options/credentials factory.
- `ddb-core/src/error.rs` (edit) — `codes::REMOTE_TIMEOUT` + `DoogatError::remote_timeout(...)`.
- `ddb-core/src/app_contract/error.rs` (edit) — one new `ErrorPolicy` row.
- `ddb-server/src/actor/mod.rs` (edit) — spawn a second worker thread; route
  `compact`/`sync`/`run_maintenance` to it; no public signature changes.
- `ddb-server/src/config.rs` (edit) — `remote_timeout_secs` (default 30, `[remote]` TOML section).
- `ddb-server/src/lib.rs` or `main.rs` (edit) — install the libgit2 global timeout opts once
  at process start, before any `DoogatService::open*`.
- `ddb-server/src/nosql_api.rs`, `docs/src/technical/server.md`, `docs/src/technical/sync.md` (edit) — doc-only.
- `tests/e2e/` (new) — `integration_maintenance_decoupling.rs` or similar module registered in `tests/e2e/main.rs`.

## Interfaces & contracts

### Remote timeout (`ddb-core/src/git_ops/remote.rs`)

```rust
/// Default bound on a remote fetch/push before it fails loud. Mirrors
/// `WRITE_LOCK_TIMEOUT`'s role for network ops; server config may override it.
pub const DEFAULT_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);

impl GitRepo {
    pub fn fetch(&self, remote: &str, branch: &str, timeout: Duration) -> Result<()> { ... }
    pub fn push(&self, remote: &str, branch: &str, timeout: Duration) -> Result<()> { ... }
}

/// One options/credentials construction point (PRD 00191 reuses it for mobile auth).
/// `deadline` = `Instant::now() + timeout`, captured once per call so every callback
/// checks the same clock.
fn remote_callbacks(deadline: std::time::Instant) -> git2::RemoteCallbacks<'static> {
    let mut cb = git2::RemoteCallbacks::new();
    let check = move || std::time::Instant::now() < deadline;
    cb.transfer_progress(move |_p| check());
    cb.sideband_progress(move |_b| check());
    cb.push_transfer_progress(move |_c, _t, _b| { if !check() { /* no abort hook here; pack_progress carries it */ } });
    cb.pack_progress(move |_s, _c, _t| { if !check() { /* abort via push_negotiation is not available; rely on transfer/sideband */ } });
    cb
}
```

**Primary mechanism, both directions: two global libgit2 options (below), not the
progress callbacks.** A "stall" per the PRD's own scenarios ("a remote that never
completes", "before progress and mid-transfer") means the socket goes quiet — no
bytes in either direction for the deadline. That is exactly what libgit2's own
idle-timeout option bounds, uniformly for fetch and push, pre- and mid-transfer, with
no dependency on which callbacks a given transport happens to fire. `fetch`'s
`transfer_progress` callback (returns `bool`, `false` aborts) is layered on top as a
secondary, fetch-only wall-clock check for the case where small progress keeps
trickling in just under the idle-timeout window forever. **Push has no abortable
progress callback in git2-rs** (`push_transfer_progress`/`pack_progress` are void
`FnMut`s) and does **not** get that secondary layer — verified against the vendored
git2 crate source; `sideband_progress` carries remote-side textual messages, not a
guaranteed per-byte progress signal during upload, so it is not relied on for abort.

This is not a gap against the PRD: `set_server_timeout_in_milliseconds` is a socket
idle bound, and socket idle is not direction-specific — it fires whenever **no bytes
move in either direction** for the deadline, which is exactly what "a remote that
never completes" / "a stalled remote" means in the PRD's fixtures. Both fetch and
push are bounded identically by it. The only scenario push's missing callback layer
leaves open is a remote that keeps the socket busy with a continuous trickle forever
(never idle, so the idle timeout never fires) — that is a different, adversarial
scenario (an actively malicious or pathologically slow peer, not "a hung remote") and
is out of this PRD's stated scope; called out explicitly in Risks so it is not
silently assumed away.

**Error mapping does not match on `git2::Error` variant at all** — it cannot: a
callback-driven abort surfaces as a generic cancelled-operation error, and a native
libgit2 connect/idle-timeout abort surfaces as its own (version/platform-varying)
network error, so matching specific `ErrorCode`s would need to track every case
libgit2 might raise for a timeout, on both triggering paths, forever. Instead
`remote.rs` wraps each `fetch`/`push` call with its own start `Instant` and timeout:
on `Err`, if `Instant::now() >= start + timeout` (with a small, e.g. 200ms, margin for
the time spent returning through the call stack), the error is reported as
`DoogatError::remote_timeout(operation, timeout)` regardless of what `git2::Error` it
actually was; otherwise the original `git2::Error` is converted and returned as
today. This covers the callback-abort path, the native connect-timeout path, and the
native idle-timeout path uniformly, with no dependency on their distinct error shapes.

**Pre-progress stall (nothing has sent a byte yet — TCP connect hang or a server that
accepts and never speaks).** No progress callback has fired yet on either direction.
Covered by the same two **global libgit2 options**
(`git2::opts`, unsafe, process-wide, set once):

```rust
// called once at process start (ddb-server main.rs / ddb-cli main.rs), before any
// DoogatService::open*; guarded by std::sync::Once — libgit2 forbids concurrent calls.
unsafe {
    git2::opts::set_server_connect_timeout_in_milliseconds(timeout.as_millis() as u32)?;
    git2::opts::set_server_timeout_in_milliseconds(timeout.as_millis() as u32)?;
}
```

`set_server_connect_timeout_in_milliseconds` bounds the TCP/TLS handshake;
`set_server_timeout_in_milliseconds` bounds socket idle time once connected (no bytes
for that long = abort). Together with the fetch-only progress-callback deadline
above, every PRD scenario is bounded: never-connects -> connect timeout;
connects-then-silent (fetch or push) -> idle timeout; fetch trickles slow progress
forever -> wall-clock deadline via the callback (push has no equivalent trickle
bound, per the Risks note above). These two calls are process-global (one timeout
value for the process), so the per-call `timeout: Duration` argument on `fetch`/`push`
is used only for (a) the fetch-only callback deadline and (b) the elapsed-time check
in the error-mapping wrapper below; the global connect/idle opts are set once from
`ServerConfig.remote_timeout_secs` (or the CLI's hardcoded `DEFAULT_REMOTE_TIMEOUT`)
at startup and not re-settable per call — a caller passing a shorter per-call
`timeout` tightens the fetch callback deadline and the error-mapping window, but
cannot tighten the global connect/idle bound itself.

**Never hold the write lease across any of this** — `fetch`/`push` take no
`with_write_lock` today and must not gain one; only `merge_remote`'s local tree/ref
writes (already lock-guarded) touch the lease, unchanged by this PRD.

### Error (`ddb-core/src/error.rs`, `app_contract/error.rs`)

```rust
pub mod codes {
    pub const REMOTE_TIMEOUT: &str = "REMOTE_TIMEOUT";
    pub const ALL: &[&str] = &[ /* ...existing..., */ REMOTE_TIMEOUT ];
}

impl DoogatError {
    /// `operation` is "fetch" or "push"; message carries no credential/URL detail.
    pub fn remote_timeout(operation: &str, timeout: Duration) -> Self {
        DoogatError::Structured {
            code: codes::REMOTE_TIMEOUT,
            message: format!("remote {operation} timed out after {}s", timeout.as_secs()),
            context: vec![("operation".into(), ErrorValue::String(operation.into()))],
        }
    }
}
```

```rust
ErrorPolicy {
    code: codes::REMOTE_TIMEOUT,
    category: AppErrorCategory::Conflict,
    http_status: 504,
    display_prefix: None,
},
```

### Maintenance worker (`ddb-server/src/actor/mod.rs`)

```rust
pub struct ActorHandle {
    tx: mpsc::Sender<ActorRun>,            // write loop (unchanged)
    maintenance_tx: mpsc::Sender<ActorRun>, // NEW: dedicated maintenance loop
    event_bus: EventBus,
    reloader: Arc<OnceLock<Arc<SchemaReloader>>>,
}

impl ActorHandle {
    pub fn spawn(repo_path: PathBuf, event_bus: EventBus) -> ActorResult<Self> {
        let _ = DoogatService::open(&repo_path)?;
        let (tx, rx) = mpsc::channel::<ActorRun>(64);
        let (maintenance_tx, maintenance_rx) = mpsc::channel::<ActorRun>(8); // small: maintenance ops are mutually exclusive by nature
        let bus = event_bus.clone();
        std::thread::spawn({ let p = repo_path.clone(); let b = bus.clone(); move || actor_loop(p, rx, b) });
        std::thread::spawn(move || actor_loop(repo_path, maintenance_rx, bus));
        Ok(Self { tx, maintenance_tx, event_bus, reloader: Arc::new(OnceLock::new()) })
    }

    /// Same dispatch mechanics as `call`, targeting the maintenance channel.
    /// `call` becomes a thin wrapper: `self.dispatch(&self.tx, event, reload, f)`.
    async fn dispatch<R, F>(&self, tx: &mpsc::Sender<ActorRun>, event: EventIntent, reload: ReloadIntent, f: F) -> R
    where F: FnOnce(&mut DoogatService) -> R + Send + 'static, R: FromActorError + IntentEmit + ReloadSignal + Send + 'static
    { /* body unchanged from today's `call`, parameterized on `tx` */ }

    pub async fn call<R, F>(&self, event: EventIntent, reload: ReloadIntent, f: F) -> R { self.dispatch(&self.tx, event, reload, f).await }

    // compact/sync/run_maintenance switch from `self.call(...)` to:
    async fn call_maintenance<R, F>(&self, event: EventIntent, reload: ReloadIntent, f: F) -> R { self.dispatch(&self.maintenance_tx, event, reload, f).await }
}
```

`compact`, `sync`, and `run_maintenance` are the only three methods that change their
dispatch target (`self.call` -> `self.call_maintenance`); every other `ActorHandle`
method, and every caller of those three (GraphQL/REST/PgWire mutation handlers,
`maintenance_loop`), is unchanged — `ActorHandle`'s public surface and signatures are
identical. `actor_loop` is reused for both threads, each opening its **own**
`GitRepo`/`Repository` handle and its own SQLite `Index` on the same path (exactly as
a second OS process would — both are designed for multi-handle access: SQLite via its
own file locking, `GitRepo` via the 00162 OS-level `ddb-write.lock`).

**NoSQL mirror handle is shared, not duplicated — this is the one part of
`DoogatService::open_shared` the two threads must NOT each call independently.**
`nosql.rs:227-228` documents that `redb::Database::create`/`open` takes a whole-file
exclusive lock; a *second* `Database::create` on the same `.ddb/nosql.redb` path from
the other thread fails outright (`DatabaseAlreadyOpen`), it does not block and wait.
Today this is a non-issue because one actor thread owns the one `DoogatService`; this
PRD's two-thread split makes it a real, demonstrable failure (the maintenance thread's
`rebuild_if_stale` after compact/sync, and the write thread's per-create/update/delete
`mirror_index_doogat`/`mirror_remove_doogat`, both touch the mirror and can now run
truly concurrently). Fix: open the `RedbMirror` **once**, in `ActorHandle::spawn`,
and construct *both* `DoogatService` instances via the existing
`DoogatService::from_parts(repo, index, shared_mirror.clone(), path)` (already `pub`,
`service/mod.rs:106`) instead of each calling `open_shared` (which calls
`default_mirror`, opening its own `Database`). `redb::Database` itself supports
concurrent transactions from multiple threads once a single handle is held (the
exclusive-open restriction is per-*handle*, not per-transaction), so one shared
`Arc`-wrapped mirror behind the existing `Box<dyn NoSqlMirrorPort + Send + Sync>` is
sufficient — no new mutex. NoSQL reads stay on the write-loop `tx` channel (the PRD's
documented, unchanged decision) — only Compact/Sync/GitMaintenance move, and now share
the write loop's mirror handle rather than opening a second one.

### Write-lease coordination

No new lock code: `svc.sync()` / `svc.compact()` already route their actual git writes
through `GitRepo::with_write_lock` (the same 00162 OS-level `ddb-write.lock` file lock
used by every other write path). Because the maintenance worker's `GitRepo` is a
*second, independent* `Repository` handle on the same `.git` directory, its write-lock
acquisition is indistinguishable (to the lock) from a second process's — it already
serializes against the write-loop's writes with no new coordination code. This is the
whole mechanism behind "write-lease coordination"; the two in-process threads look
exactly like two processes to `write_lock::acquire`.

### Config (`ddb-server/src/config.rs`)

```rust
pub struct ServerConfig {
    // ...existing...
    pub remote_timeout_secs: u64, // default 30
}
// [remote] section, `timeout_secs`, mirrors the existing [maintenance] pattern.
```

## Data flow

1. A client mutation (GraphQL/REST/PgWire) or a NoSQL read dispatches through
   `ActorHandle::call` -> write-loop channel -> write-loop's own `DoogatService`.
2. A `Sync`/`Compact`/manual-`GitMaintenance` request (client-triggered or
   `maintenance_loop`'s periodic tick) dispatches through the new
   `ActorHandle::call_maintenance` -> maintenance channel -> maintenance worker's own
   `DoogatService`.
3. Inside the maintenance worker, `svc.sync()`'s fetch/push calls run with the
   configured `Duration` deadline (global libgit2 opts bound connect/idle for both
   directions; fetch additionally bounds a trickling transfer via the
   `transfer_progress` callback); on expiry (judged by elapsed time against the
   configured timeout, not by matching a specific `git2::Error` shape — see
   Interfaces) the call returns `DoogatError::remote_timeout`, which the maintenance
   worker returns to its caller exactly like any other `ActorResult` error — no
   retry, no background continuation.
4. Any actual local git write inside either thread's `with_write_lock` call blocks on
   the shared OS lock file until the other side releases it (bounded by
   `WRITE_LOCK_TIMEOUT`, unchanged), then proceeds from `fresh_index()` as today.

## Reuse inventory

- `GitRepo::with_write_lock` (`ddb-core/src/git_ops/mod.rs:337`) — reused as-is for
  write-lease coordination; no new lock primitive.
- `actor_loop` (`ddb-server/src/actor/mod.rs:580`) — reused verbatim for the second
  thread; no new loop function.
- `DoogatService::from_parts` (`ddb-core/src/service/mod.rs:106`, already `pub`) —
  reused to build the maintenance worker's service around a shared NoSQL mirror
  handle, rather than opening a second one.
- `ServerConfig`'s `[maintenance]` TOML pattern (`config.rs:21-25,63-73`) — copied for
  the new `[remote]` section rather than inventing a new config shape.
- `app_contract::error::ERROR_POLICY` table (PRD 00173, just shipped) — the single row
  this PRD adds is the intended extension point; no parallel error path created.
- Searched (nothing found): `rg -n "server_timeout|connect_timeout|RemoteCallbacks" ddb-core/src ddb-server/src` →
  no existing timeout/callback wiring anywhere in the tree; `rg -n "FetchOptions|PushOptions"` →
  none; confirms the PRD's own claim that `remote.rs` installs no options today.

## Alternatives considered

1. **Smallest diff: timeout only, no worker split.** Add the remote deadline (Phase 0)
   and leave sync/compact on the write-loop thread. Rejected as the sole fix: a fast
   local sync with a slow merge-apply would still stall mutations, and the PRD's
   primary success metric ("an independent create completes during a blocked sync")
   is unmet without the worker split.
2. **Shared `Mutex<DoogatService>` across one worker pool** instead of two dedicated
   single-thread loops. Rejected: `DoogatService`/`GitRepo` hold `Cell`s and are
   designed as single-owner-per-thread (see `write_lock_depth`'s doc comment); wrapping
   one instance in a `Mutex` and sharing it between the write loop and a maintenance
   task would serialize them anyway (defeating the decoupling) while adding lock
   contention the OS-level write lock already provides for free.
3. **Chosen: two independent `actor_loop` threads, each with its own `DoogatService`,
   coordinating only through the existing 00162 file lock.** Zero new concurrency
   primitives, reuses the exact mechanism that already serializes the CLI against a
   running server; the only new code is the channel-selection branch in three
   `ActorHandle` methods.

## Risks & edge cases

- **Maintenance worker starvation**: a long sync queued behind a long compact on the
  single maintenance channel (capacity 8) delays the next maintenance request, but
  never delays mutations — matches the PRD's accepted mitigation ("maintenance ops are
  already mutually exclusive").
- **Push has no wall-clock layer beyond the idle timeout**: a continuously-trickling
  push (never fully idle) is not bounded by this design; only the stalled/hung-remote
  scenarios the PRD describes are in scope. If a future PRD needs that adversarial
  case closed, it needs a mechanism beyond git2-rs callbacks (e.g. a custom transport
  or a watchdog thread with a true cancel path), not a widening of this design.
- **libgit2 global opts timing**: `git2::opts::set_server_*_timeout_in_milliseconds`
  must run before any thread opens a `Repository` and must not race another call;
  guard with `std::sync::Once` at the single process-startup site. Calling it from a
  test harness that also runs other git2-using tests in the same process could race —
  tests for this PRD should assert behavior via the per-call progress-callback
  deadline (deterministic, no global state) wherever possible, reserving the global
  opts for the one "connection never establishes" fixture.
- **Likely next changes**: (1) the deferred shared read-only redb handle for NoSQL
  `ReadPool` migration (follow-up noted by the PRD); (2) PRD 00191 (FFI git sync /
  mobile replica) reuses `remote_callbacks`'s options/credentials factory — keep it as
  the single construction point so that PRD doesn't need a second one; (3) a future
  per-request remote-timeout override (today's deadline is process-global for the
  connect/idle bound) — if ever needed, thread a `Duration` through to the global opts
  call per-sync is NOT safe (concurrent sync+fetch would race-set the global), so that
  follow-up would need a different mechanism (e.g. a libgit2 custom transport) rather
  than widening this design's opts call.

## Test strategy outline

- **Core (`ddb-core`)**: a local TCP listener that accepts and never responds (connect
  succeeds, then silence) proves the idle-timeout opt aborts `fetch`/`push` within the
  configured bound; a fetch-only slow-but-progressing fixture (sends bytes just under
  the idle timeout, forever) proves the `transfer_progress` wall-clock deadline aborts
  it (push has no equivalent fixture — documented residual gap);
  `remote_timeout` error maps through `ERROR_POLICY` to `Conflict`/504/`REMOTE_TIMEOUT`
  (reusing the existing error-policy registration test pattern from PRD 00173, which
  already asserts every `codes::ALL` entry has a row).
- **Server integration (`tests/e2e/`)**: a stubbed slow sync (local bundle/remote
  fixture that stalls mid-merge) running on the maintenance worker while a concurrent
  `createDoogat` is issued — assert the create completes within a bounded time
  (e.g. well under the sync's artificial stall). A concurrency test issuing a
  maintenance-worker write and an actor write at the same time asserts both land with
  no lost update (the 00162 lease serializes them). A NoSQL GET issued during an
  in-progress compact asserts bounded latency (closes the PRD's Phase 2 acceptance).
- Tier 1 gate unchanged: `cargo build`, `cargo clippy --workspace --all-targets`,
  `cargo test-ci`; the new e2e scenarios are authored for Tier 2 per CLAUDE.md.

## Review log

- non-blocker (dispatch 1): maintenance channel capacity (8) is an unexplained constant beyond "small on purpose" — any value works since overflow only backpressures, no correctness impact; left as-is.
- non-blocker (dispatch 1): two independent `Repository` handles on the same `.git` dir can see stale mmap'd pack/odb state relative to each other during compaction — pre-existing class shared with today's CLI-vs-server concurrency (not new to this PRD), not addressed further here.
- question (dispatch 1): confirm `sync_manager`'s orchestration never calls `push` from inside a `with_write_lock` closure (composition could violate the "never hold the lease across network I/O" invariant even though `remote.rs` itself holds no lock) — verify at implementation time, not re-derived from reading `remote.rs` alone.
- question (dispatch 1): undecided what happens if the linked libgit2 predates `set_server_connect_timeout_in_milliseconds`/`set_server_timeout_in_milliseconds` and the call errors at startup — implementation must pick hard-fail-startup vs. log-and-continue-unbounded explicitly rather than let `?` decide implicitly.
- dispatch 1 (claude): cardinal-sin 0, blocker 2, non-blocker 2, question 2
- dispatch 2 (codex): push-timeout blocker fixed (idle-timeout is bidirectional, not push-exempt; wording corrected, residual trickle-only gap kept explicit in Risks, not claimed closed). Codex's own repo/dependency verification was blocked by its sandbox this run (`sandbox_apply: Operation not permitted`), so this dispatch's finding was evaluated on the design text's internal logic alone, not against fresh evidence from the referenced source files.
- dispatch 2 (codex): cardinal-sin 0, blocker 1, non-blocker 0, question 0
- dispatch 3 (codex): error-mapping blocker fixed (dropped the `ErrorCode`-matching sentinel approach entirely; fetch/push now wrap each call with an elapsed-time check against the configured timeout, so a native connect/idle-timeout abort and a callback-driven abort both map to `remote_timeout` the same way, with no dependency on libgit2's error shape for either). Non-blocker (leftover push-wall-clock wording in three spots) also fixed — "every PRD scenario is bounded" / the per-call-deadline paragraph / Data-flow step 3 now say fetch-only explicitly. Codex's sandbox again blocked `remote.rs`/`Cargo.lock` reads this run (`sandbox_apply: Operation not permitted`); it reviewed doc-internal logic only and said so.
- dispatch 3 (codex): cardinal-sin 0, blocker 1, non-blocker 1, question 0
