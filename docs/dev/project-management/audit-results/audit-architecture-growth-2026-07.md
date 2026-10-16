# ddb architecture audit — growth & stability (2026-07-07)

Five parallel adversarial audits (core architecture, transport-parity cost, jink
downstream, operational safety, security). Load-bearing claims independently
verified against source before inclusion. `UNVERIFIED` marks anything not
confirmed read-only.

## Verdict

**Save it, don't rewrite.** The foundation is sound: git-as-source-of-truth with
a rebuildable index, three-zone parsing, the port traits, and the AppError shape
all hold under scrutiny. The hardest distributed-systems piece — cross-device
same-second ID collision *at sync time* — is handled correctly. Domain types are
genuinely adapter-free. Panic discipline is strong. Docs match the code. Every
serious finding is at a seam or an edge, not in the model. A rewrite would delete
the only proof of correctness you have (e2e, property tests, a production
downstream) and reintroduce these same bug classes uncaught — six dark months
against impatient investors. Fix-forward wins.

**But** the operational-safety audit found a genuine data-loss class that is NOT
in the backlog and is triggered by the exact deployment the product is built for
(a downstream app running `ddb serve` while the user also runs the CLI, or a
mobile app syncing). That raises *urgency*, not the rewrite question — git
storage means even these are recoverable and fixable in place.

---

## The emergency (not in backlog, verified)

### C1 — Concurrent git writes silently lose commits. [CRITICAL, verified]
`ddb-core/src/git_ops/mod.rs:292-322` (`commit_files`) and `:342-393`
(`delete_file`/`delete_files`): build a tree from the current index, resolve HEAD
as parent, and `repo.commit(Some("HEAD"), …)` with no cross-process lock, no
compare-and-swap on the ref, and no retry. There is no advisory lock file
anywhere (only in-process mutexes; the sole cross-process guard is
`BEGIN IMMEDIATE` around SINGLETON writes).

Scenario: `ddb serve` actor and a CLI `ddb create` write in the same window.
Process 2 resolves parent=C1 (P1's commit) but its tree was built from H0 →
P1's doogat is silently absent from the new HEAD. Worse, `git maintenance --auto`
fires from `check_write_threshold` (`maintenance.rs`) on every Nth commit and
prunes the now-unreachable commit **permanently**. No error surfaces.

Fix: one repo-scoped OS file lock (`fs2`/`fd-lock` on `.git/ddb-write.lock`) held
across write→stage→commit in every `commit_*`/`delete_*`; or CAS the ref from the
expected old OID and rebuild-tree-and-retry on mismatch. One flock removes the
whole class.

### H1 — Conflicted merges silently revert the other side's deletions. [HIGH, verified]
`ddb-core/src/git_ops/merge.rs:116-119`: `collect_theirs_only_changes` stages
only `Delta::Added | Modified` and `continue`s past `Deleted`. Device B deletes
doogat Y and edits X; device A also edits X → conflict on X → the merge tree
still contains Y → Y is resurrected on every node, no marker, no log. This
bypasses the intentional delete-vs-edit resurrect policy in
`sync_manager/mod.rs:427-456`. Renames (Deleted+Added) leave the old path alive →
duplicated doogat. Fix: include `Delta::Deleted` (remove from index).

### H2 — Delete uses the stale cached git index. [HIGH, verified]
`ddb-core/src/git_ops/mod.rs:349,377` use `self.repo.index()` while every write
path uses `fresh_index()` — whose own docstring says the cache "can silently
revert those changes on the next commit." Server deletes G right after a CLI
committed F → the delete's tree is built from the pre-F snapshot → commit removes
G *and* F. Fix: use `fresh_index()` in both delete paths.

### ID minting — two unguarded paths + future-dated batch IDs. [HIGH, verified]
`ddb-core/src/parser/mod.rs:933-947`: `generate_unique_id` dedups via a
**per-process** static and mints from `chrono::Local`. `service/create.rs:429`
(raw FFI create) and `discovery.rs:89` mint with `exists = |_| false` (no repo
check). `sql_engine/mod.rs:127-168` (`unique_ids`) hands out **future** timestamps
(+1s per row) for batch INSERTs, checked only against the index. A single create
landing in a second a batch already claimed resolves to the same `ddb/{id}.md`
and commits over it. Fix: route all minting through one repo-aware `unique_id`;
stop allocating future timestamps.

---

## Convergence / determinism (not in backlog, verified)

### H3 — CRDT resolution is non-deterministic. [HIGH, verified: no `set_actor` anywhere]
`ddb-core/src/crdt_resolver.rs` never calls `set_actor`. When ours and theirs set
the same frontmatter scalar (status, priority, any typed column) or insert at the
same text position, Automerge breaks the tie by actor ID — freshly randomized per
invocation. Two nodes independently resolving the same conflict (routine: the
loser of a push race has already committed its own resolution) pick **different
winners** → extra merge rounds, and a final value that can differ from what either
node showed the user. Fix: `doc.set_actor(hash(blob_oid))` so resolution is a pure
function of inputs. Two lines.

### M1 — HLC is vestigial for the decisions it supposedly governs. [MED]
No HLC trailer is written by `commit_file`/`commit_files` (only conflict/sweep
commits get one), so `find_hlc_for_path` returns None almost always. Consequences:
add-add "later HLC wins" is really "theirs wins"; `resolve_lww` fallback is "ours
wins" — **opposite defaults on different paths**, so symmetric concurrent
resolutions pick opposite winners. `recv_hlc` is dead code outside tests → a
device hours ahead stamps future HLCs that beat honest edits. `Hlc::now` itself is
correctly monotonic. Fix: write HLC trailers on all commits, call `recv_hlc` on
merge, unify the missing-HLC tie-break direction.

### H4/H5 — Sync commit-sequencing gaps. [HIGH]
H4: collision-loser reassignment is a *separate later commit*
(`sync_manager/mod.rs:329-332,676-711`); crash between winner-commit and
loser-reassign strands the loser at no path in HEAD. H5: conflicted `git bundle`
import leaves the repo mid-merge, `resolve_post_merge_conflicts` sees
`parent_count < 2` and reports success with 0 conflicts while nothing merged, then
deletes the bundle ref → auto-gc prunes the offline-synced data. Fix: fold
winner+loser+link-rewrites into one merge tree; drive the conflict pipeline after
a conflicted bundle merge and only delete the ref on a successful merge commit.

---

## Availability (mostly not in backlog, verified)

### Poison file aborts incremental reindex. [MED→HIGH, verified, found by 2 agents independently]
`ddb-core/src/indexer/rebuild.rs:182-194` (`batch_index_changes`): `content?` and
`parser::parse(...)?` both propagate. One non-UTF-8 or bad-frontmatter file in a
sync change-set fails the whole batch → the *other* valid changes aren't indexed
either. `ensure_fresh` (`service/mod.rs:116`) gates ~40 entry points on this, so a
single stray file (users edit these `.md` files with other tools; sync brings
untrusted content) turns every read *and* write into an error until a manual full
rebuild. The **full** rebuild path (`parallel_parse`) already collects warnings
instead of failing — the hot incremental path is the inconsistent one. Directly
violates the "must not fail on parse failures" guardrail. Fix: align
`batch_index_changes` with `parallel_parse` (warn-skip into `RebuildReport`).

### No GraphQL depth/complexity limit. [MED, verified structurally]
`schema/mod.rs:53-95` builds the dynamic schema with no `.limit_depth()`,
`.limit_complexity()`, or `.disable_introspection()`. Every query funnels through
the single actor thread. One authed client (or a compromised downstream token) can
send a deeply nested relation/backlink query and pin the actor, stalling all
clients. Fix: `builder.limit_depth(15).limit_complexity(500)`.

### Actor head-of-line blocking. [MED]
`actor/handlers.rs:127-162` runs `Compact` and `Sync` inline on the single actor
thread; a slow/hung git remote stalls every mutation on every transport (channel
cap 64, then HTTP back-pressures). No timeout found around remote ops. NoSQL HTTP
reads also route through this actor instead of the concurrent ReadPool
(`nosql_api.rs`→`handlers.rs:11-16`) — a long compact stalls all NoSQL GETs. Fix:
run sync/compact on a maintenance thread with a write lease; bound remote ops with
timeouts; move NoSQL reads to ReadPool (redb single-thread requirement UNVERIFIED).

---

## Security (new, beyond backlog)

Traversal surfaces are **closed** (write/read/rename/attachment all validated,
FTS5 parameterized, token secrecy handled, error redaction works on
GraphQL/REST/NoSQL). Remaining:

- **Materializer/delete DDL identifiers unescaped. [MED, verified unescaped; exploit reachability UNVERIFIED]** `indexer/materialize.rs:96-167` and
  `service/delete.rs` interpolate `table_name`/column/`dtype` bare-quoted with no
  `escape_sql_ident` (which *is* applied in the where-filter and schema-apply
  paths). Names derive from attacker-influenceable `type:` frontmatter and field
  keys. A `"`-bearing name breaks the quoting. Bounded (single-statement, no
  stacked DROP) → DoS/corruption, not exfiltration. Distinct from the already-fixed
  schema-apply path. Fix: wrap every identifier in `escape_sql_ident`; validate
  `type`/field-key charset at the write boundary.
- **PgWire leaks unredacted internal errors. [MED]** `pgwire.rs:107,114` ship
  `Box::new(e)` raw — `git: …`/`io: …` detail crosses the wire that GraphQL
  redacts. Fix: route through `classify` first.
- **Non-constant-time token compare. [LOW]** `auth.rs:30-34` uses `==`. Fix:
  `subtle::ConstantTimeEq`.
- **Non-local bind, no warning/TLS. [LOW]** `--bind 0.0.0.0` is accepted silently;
  the bearer token then crosses the network in cleartext. Fix: warn on non-loopback
  bind.
- No explicit request body-size limit; PgWire SELECT has no row cap; `serde_yaml`
  is unmaintained with no frontmatter size cap. [LOW each]

---

## Throughput ceiling (verified) — needs a product decision now

`generate_unique_id` (`parser/mod.rs:933-947`) holds a global mutex and
`thread::sleep(100ms)` until the wall-clock second advances → **~1 create/sec per
process**, and the ID *is* the public contract (filenames, wikilinks). The tree
already contains an HLC that isn't used for identity. This is the single most
expensive thing to change later. Decide now: bless HLC-derived/suffixed IDs before
more data accretes, or document 1/sec as a permanent ceiling in design-decisions.

---

## Backlog items — validated and reprioritized

- **00162 (unify error mapping): CONFIRMED but UNDERSCOPED.** It's not two paths,
  it's **four**: `classify` (GraphQL/REST/NoSQL), `http_error.rs` status table,
  `AppError` categories, FFI's own allowlist (`ffi/records.rs:111-126`, which
  lossily collapses `SINGLETON_NOT_FOUND`→Validation, `Conflict`→`Git`), and PgWire
  which carries no code at all. The missing `SqlEngine` arm in
  `From<DoogatError> for AppError` (`app_contract/error.rs:49`, verified) means a
  SQL syntax error becomes `SQL_ERROR`+message on one path and redacted
  `INTERNAL_ERROR` on another. Scope 00162 to a single code→(category, status,
  redaction) table in core that all five transports derive from.
- **00163 (where-filter correctness): real.** Null-quantifier footgun confirmed.
- **00164 (split oversized modules): real.** 9 files exceed the 800-line cap;
  `ddb-server/src/filter.rs` is 3,397 lines of query policy living in a transport
  crate (should move next to `search_query.rs` in core).
- **00165 (schema-apply v2 + FFI zone parity): real.** Ties into the FFI-parity
  theme below.

---

## The meta-finding — the real leverage (this is the investor answer)

Six of the biggest findings share **one root cause**: the commands/results "app
contract" was designed but only half-built.

- It covers **3 of ~10 verbs** (create/update/applySchema). `ReadCommand`,
  `DeleteCommand`, `SearchCommand` have **zero consumers** — dead code
  (`app_contract/results.rs:3-6` says so outright).
- **FFI bypasses it entirely** — `driver.rs:66-94` take raw Markdown strings, so
  FFI clients never get warnings, conflict policy, typed fields, or defaulting, and
  every contract improvement needs a *new* FFI method + Swift/Kotlin regen.
- **Error mapping fans into 4 tables** because there's no single policy object.
- **~1,000 LOC of the server is pure actor plumbing** (36 command + 27 reply enum
  variants + 4 hand-threaded touch points per verb) because each verb is wired by
  hand.
- **Result shapes have already drifted**: FFI `SearchResult` exposes 6 of 10 core
  fields, REST 4; REST silently drops search filters; three transports enforce
  different unregistered-type policies.
- **jink hand-rolls a client** because there is no blessed one.

Finish the contract — route *every* verb (including FFI) through
commands/results, make the error policy one table, replace the twin actor enums
with a closure-carrying message — and transports collapse to thin adapters, error
mapping unifies (absorbs 00162), FFI reaches parity, and a **generated client
becomes possible**. That last point is the bridge to the downstream story.

The encouraging signal: the newest work (schema-apply) is already the clean
end-to-end instance of this pattern. The codebase is converging on the right
shape; it just hasn't been retrofitted backward.

---

## Downstream (jink) — deleting the repetitive coding

jink talks to ddb over GraphQL only. Roughly **2,400 LOC of its backend + ~1,250
LOC of its frontend is generic ddb glue or gap-workarounds**, versus ~600 LOC of
real domain logic. The biggest chunks and the upstream feature that deletes each:

1. **Published `ddb-client` Rust crate** (typed error codes, variables-only
   builders, connection/nested-payload parsing, `wait_ready()` backoff, token
   discovery). Deletes jink's `ddb_client.rs` (~226 + ~540 test LOC) + parse glue
   at every call site. jink's own architecture doc already specifies this client —
   proof the need is generic, not jink-specific.
2. **Structured `fields` input + typed values** on generic mutations — accept a
   JSON *object* and native numbers/booleans, not a JSON-encoded *string*. Deletes
   every `build_fields_json` / `.to_string()` blob and the "boolean needs the
   string 'true'" workaround. Every downstream hits this day one.
3. **Typed-query completeness** — date `orderBy`, relation/negation `where:`,
   junction traversal in `search()` (overlaps 00163). Deletes jink's client-side
   sort/pagination fallback (which is also a real **pagination-correctness bug**
   today), category pre-resolution, and two-step junction resolvers (~300 LOC).
4. **Rust codegen from typedefs + offline SDL export** (`ddb schema sdl`). Deletes
   jink's hand-mirrored structs for all 7 types and unblocks frontend codegen
   without a running token-authed server.
5. **`ddb-testkit` crate or `ddb serve --ephemeral`** — tempdir init, free-port
   spawn, health poll, teardown. Deletes ~200 LOC of `tests/common/mod.rs` that any
   downstream rewrites identically.
6. **Idempotent upsert / mutation responses that obviate re-reads** — an
   `onConflict: RETURN_EXISTING`. Deletes the UNIQUE_VIOLATION-catch-then-refetch
   and create-then-re-read dances. ddb's own code comments already note this.
7. **Version/capability handshake** on `/health` — replaces jink's per-feature
   live-probing policy; would have caught the 0.2.6 search-semantics break before
   runtime.

Correctness pain jink actually absorbed (evidence in ddb CHANGELOG + jink code):
silent partial writes (createDoogat only wrote the base row), error codes
regressing twice, wrong ID from `createMany(onConflict: IGNORE)`, a breaking
search-semantics change in 0.2.6, no parameterized SQL (jink maintains its own
escaping + injection test suite). These are the "can they deliver" signal the
investors are reacting to — each one is a downstream fire lit by an upstream gap.

---

## Rescue plan (sequenced)

**Phase 0 — stop the bleeding (days, not weeks).** C1 write lock (one flock kills
a whole data-loss class), H1 (`Delta::Deleted` in merge), H2 (`fresh_index()` in
delete). Small diffs, permanent-data-loss class.

**Phase 1 — convergence & correctness.** H3 stable actor IDs (2 lines), poison-file
warn-skip, ID-minting unification + the 1/sec throughput decision, M1 HLC trailers,
H4/H5 sync commit-sequencing.

**Phase 2 — finish the app contract (the leverage play).** Wire all verbs through
commands/results, route FFI through it, unify error policy (absorbs 00162), replace
the actor twin-enums with a closure message. Transports become thin; parity stops
being hand-enforced.

**Phase 3 — downstream enablement (the investor demo).** `ddb-client` crate + SDL/
codegen export + structured `fields` + `ddb-testkit`. Deletes ~2/3 of jink's
integration code — a visible, measurable win.

**Phase 4 — hygiene.** Module splits (00164), where-filter (00163), GraphQL depth
limits, materializer identifier escaping, remaining security hardening.

Phases 0-1 are the "we are not in bad shape" proof. Phase 2 is the structural
answer to "cost of change is too high." Phase 3 is the answer to "downstreams
repeat too much code." Phase 4 is cleanup.

---

## What the architecture gets right (do not "fix" these)

Domain types are adapter-free. Cross-device same-second ID collision at sync is
handled properly (winner/loser reassignment, not franken-merge). Index
materialization warns instead of dropping corrupt rows. Server read/write split is
sound (single-writer actor + semaphore-bounded ReadPool). `AppError` is the right
target shape. Structured create checks the filesystem before minting. Panic
discipline is strong (9 `panic!`/`unreachable!` outside tests). Repo-format
versioning rejects newer-repo opens and auto-migrates old ones. Compaction does
**not** rewrite history and takes a backup bundle first. Docs track reality
unusually well. SQLite is WAL + busy_timeout on every connection.
