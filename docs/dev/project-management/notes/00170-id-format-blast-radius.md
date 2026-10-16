# 00170 — ID-format blast-radius research note

Evidence trail for the Phase 0 research task of `dev/local/prds/wip/00170-id-throughput-ceiling-decision-v1.md`.
Extends (does not re-derive) the mint-path characterization already written into
that PRD's Problem Statement and into the `id_minting.rs` module doc comments.

Premise (already decided, not re-litigated here): the maintainer's 2026-07-13
pre-decision fixes the outcome direction — minting moves to UTC with a node
discriminator, so a cross-device collision stops being routine. It does **not**
become impossible: every option below sources its discriminator from
`truncate_node` (`ddb-core/src/hlc.rs:123`-`:125`), which keeps only the first 8
non-dash characters of the node UUID — 32 bits — so two nodes whose UUIDs share
that prefix get an identical node field (2^-32 per node pair; ~50% somewhere in a
population of ~77,000 nodes). Every option below is therefore
collision-*resistant* at mint, never collision-free, and
`resolve_add_add_collision` + `derive_content_id` stay the backstop for the
residual under all of them. Option (a) "keep second-resolution" is off
the table as the chosen outcome; it is recorded below only for why it lost. The
live choice is between (b) sub-second suffix/counter and (c) HLC-derived identity.

## Baseline: the ~1/sec/process ceiling

`generate_unique_id` (`ddb-core/src/parser/mod.rs:954`-`:968`) holds a
process-global `static LAST: Mutex<String>` (`:956`), formats a candidate from
`chrono::Local::now()` at second resolution (`:960`), and when the candidate
equals the last-minted value or the caller's `exists` oracle reports it taken,
`std::thread::sleep(Duration::from_millis(100))` (`:966`) and loops. Only one
distinct candidate string exists per wall-clock second, so no matter how often
the 100ms loop polls, at most one mint succeeds per second — **~1 mint/sec
per process**. This is the number every option below is compared against.

**Unit discipline for the whole note**: every throughput figure below is a rate of
**ID mints**, not of doogat creates. The two happen to coincide today (~1/sec)
because the mint throttles at one per second *and* a create writes a git commit
that costs about as much — `docs/src/technical/performance.md:87` measures a
single doogat create at 971.97 ms and `:95` states "Create latency is dominated
by git commit per doogat (~1s each)". They stop coinciding the moment the mint
throttle is removed: mint throughput jumps by orders of magnitude while
end-to-end create throughput stays at ~1/sec, still bound by that commit. No
option below changes the commit cost.

The unified repo-aware oracle that supplies `exists` to every mint path is
`id_minting::existence_oracle` (`ddb-core/src/id_minting.rs:24`-`:35`): it
snapshots the repo HEAD tree (via `parser::extract_id_from_path`,
`ddb-core/src/parser/mod.rs:972`-`:977`) union the `doogats` SQLite index into
one `HashSet<String>` and returns a pure membership closure — one repo/index
read per call, not per candidate.

Consumers, and where the ceiling bites:

- `DoogatService::unique_id` (`ddb-core/src/service/write_helpers.rs:244`-`:247`)
  — one `existence_oracle` build + one `generate_unique_id` call per doogat
  create. The ceiling bites **per call**, i.e. per created doogat, for this path.
- `SqlEngine::unique_ids` (`ddb-core/src/sql_engine/mod.rs:126`-`:150`) — builds
  the oracle once, mints one seed ID via `generate_unique_id` (which may still
  spin-wait), then for the rest of a batch **steps the timestamp by `+1s` per
  additional row** instead of re-calling the sleeping mint
  (`ddb-core/src/sql_engine/mod.rs:138`-`:147`). This sidesteps the 100ms
  spin-wait for `count - 1` of the `count` rows, so a SQL batch `INSERT`
  effectively bites the ceiling **once per call site, not once per row** — the
  batch is still second-spaced (so a 10-row batch still spans ~10 wall-clock
  seconds of ID-space even though it doesn't literally sleep for each row).
- Collision-loser reassignment during sync/merge (see "Sync collision
  behavior" under option (a) below) does **not** go through
  `existence_oracle`/`generate_unique_id` at all; it uses the separate,
  deterministic `id_minting::derive_content_id` (`ddb-core/src/id_minting.rs:71`-`:97`),
  so it is unaffected by the ~1/sec ceiling in either direction.

**Correction to the PRD's code citation**: the PRD text and this task's Code-to-trace
list name `SyncManager::reassign_single_loser` at
`ddb-core/src/sync_manager/mod.rs:694`. That symbol does not exist in the
current tree — line 694 falls inside `detect_stale_nodes`, unrelated to
collision reassignment (verified by reading the file; no `reassign_single_loser`
symbol anywhere in `sync_manager/mod.rs`). The actual collision-loser path in
this file is the free function `resolve_add_add_collision`
(`ddb-core/src/sync_manager/mod.rs:135`-`:198`), which builds a
`CollisionLoser` (`:187`-`:195`) carrying `old_id` and `losing_blob_oid`; per
`id_minting.rs`'s own doc comment (`:70`), the actual reassignment call site —
consuming that `CollisionLoser` to invoke `derive_content_id` — lives in
`git_ops::merge::fold_losers_into_index`, a file outside this task's read
scope. This note cites the code it could verify (`resolve_add_add_collision` +
`derive_content_id`) in place of the stale symbol name; likely drift from a
refactor after the PRD text was drafted.

## HLC background (candidate identity source for option c)

`Hlc` (`ddb-core/src/hlc.rs:10`-`:14`) is `{ wall_ms: u64, counter: u32, node: String }`
(node = first 8 chars of a node UUID, `truncate_node`, `:123`-`:125`).
`Hlc::now` (`:18`-`:44`) is a pure in-memory computation — no I/O, no clock
sleep-wait, just wall-clock read + counter bump when two calls land in the same
millisecond. `Display` renders `{wall_ms}-{counter:04}-{node}` (`:110`-`:114`,
e.g. `1709000000000-0042-abcd1234`) — **`wall_ms` is not zero-padded to a fixed
width**, only `counter` is (`:04`).

The machine-local persisted `HlcClock` (`ddb-core/src/git_ops/hlc_clock.rs`)
wraps `Hlc` with cross-process durability: `tick()` (`:69`-`:75`) reads the
persisted floor, computes the next `Hlc::now`, and **synchronously writes +
renames `<git_dir>/ddb-hlc` on every tick** (`persist`, `:94`-`:103`) so a
fresh short-lived CLI process doesn't regress. This per-tick disk I/O is the
mechanism that already stamps every write commit's HLC trailer.

## Option (a): keep second-resolution — why it lost

Not re-derived in blast-radius depth (per task scope); recorded for the
record only. The maintainer's 2026-07-13 pre-decision overrides it: the defect
is identity correctness, not throughput. Under local-time, second-resolution
minting, two devices in different timezones (or across a DST fold) can
independently mint the **identical** ID with zero cross-device coordination —
`generate_unique_id`'s `exists` oracle only ever sees local repo/index state at
call time, never a remote device's concurrent-but-uncommitted mint. That
collision is real today and is only caught reactively, at merge time, by
`resolve_add_add_collision` + `derive_content_id` (see below). "Keep" would
mean permanently accepting that reactive-only collision story instead of
minting with a per-node discriminator from the start. Throughput was never the deciding
factor; it is a side effect of the same fix.

- **Filename stability**: compatible as-is — `ddb/{id}.md`, no change.
- **Wikilink rewrite**: none needed — no format change.
- **Lexical-sort assumptions**: holds — fixed-width zero-padded `YYYYMMDDHHmmss`, lexical order == chronological order.
- **Sync collision behavior**: reactive only. Two devices CAN mint the same ID (local-time, no cross-device coordination at mint time); collisions are detected only at merge, resolved by `resolve_add_add_collision` picking a winner via `lww_pick` and reassigning the loser's ID deterministically through `derive_content_id` (seeded by `(old_id, losing_blob_oid)`, so both sides independently compute the identical new ID with no coordination). This is exactly the defect the maintainer's pre-decision rejects.
- **Downstream ID parsing**: unaffected — no format change.
- **Throughput**: **~1 create/sec/process** (current, measured behavior of `generate_unique_id`; this is the baseline every other option is compared against).

## Option (b): sub-second suffix/counter

Shape per the PRD framing: retain the 14-digit `YYYYMMDDHHmmss` prefix,
append a lexical-sort-preserving suffix (per the maintainer's UTC + node
discriminator direction, the suffix must itself encode a per-node
discriminator, not merely a local counter — a bare local counter without a
node component would let two devices collide again in the same millisecond,
reopening exactly the defect option (a) lost on).

- **Filename stability**: `ddb/{id}.md` stays compatible in *mechanism* — `extract_id_from_path` (`ddb-core/src/parser/mod.rs:972`-`:977`) takes the file's whole stem via `file_stem()`, with no length or digit-count validation at that call site, so a longer stem works as a filename with no code change there. But the *convention* changes: AGENTS.md documents `DoogatId` as "a 14-digit timestamp string," and the internal shape check exercised by `id_minting.rs` tests requires "exactly 14 ASCII digits" (`DoogatId::is_valid_shape`, referenced at `ddb-core/src/id_minting.rs:401`-`:409`). A longer ID fails that exact-length invariant unless it is relaxed — a real, if mechanical, code change outside this task's scope (owned by the follow-up implementation PRD).
- **Wikilink rewrite**: no rewrite of *stored* content is forced. `extract_wikilinks_from`'s regex (`ddb-core/src/parser/mod.rs:518`, `\[\[([^\]|]+)(?:\|([^\]]+))?\]\]`) and `rewrite_wikilinks` (`:566`-`:586`) match any non-empty, non-`]`/`|` run as the target — no digit-count or length constraint. Old 14-digit-only `[[<id>]]` links keep resolving unchanged (the 14-digit prefix IS a valid old ID), and new longer IDs are equally matchable by the same regex. No forced migration.
- **Lexical-sort assumptions**: holds *only if* the suffix is fixed-width and zero-padded (this is what "lexical-sort-preserving" in the contract requires) so that string order continues to equal chronological + causal order. This is a design constraint the follow-up PRD must pin, not something the current code enforces.
- **Sync collision behavior**: collision-*resistant* at mint time, *if and only if* the appended discriminator is a genuine per-node identifier (e.g. sourced the same way `HlcClock` sources `node_id` from `<git_dir>/ddb-node`, `git_ops/hlc_clock.rs:42`-`:46`) rather than a purely local counter. Two devices then mint effectively disjoint ID spaces even within the same wall-clock second, and the reactive `resolve_add_add_collision`/`derive_content_id` merge-time path becomes a backstop rather than the only defense. It does **not** become unnecessary: with `truncate_node`'s 8-character (32-bit) field, two nodes whose UUIDs share those characters mint from the *same* space and collide whenever they also share a sub-second slot.
- **Downstream ID parsing**: a consumer doing a **prefix parse** of the first 14 characters keeps working unchanged; a consumer doing a **whole-string exact-match** against a 14-digit-only shape breaks. Verified rather than assumed, 2026-08-16: **jink does not parse the ID at all.** It types every doogat id as an opaque GraphQL `ID` scalar — `frontend/src/lib/graphql/generated.ts:740` (`QueryDoogatArgs.id`, the lookup key) and `:1036` (a returned id) — and a search of the whole jink checkout (Rust backend and Svelte frontend, `node_modules` excluded) finds no id slicing, no length or digit-count check, and no date-from-id parsing; its three `slice(`/`substring(` frontend uses are array pagination and a generic string-truncation helper, none of them applied to an id. jink's *only* `14-digit` reference is a generated doc comment at `generated.ts:642`, and that string is not jink's: it is copied verbatim from ddb's own GraphQL schema description at `ddb-server/src/schema/queries.rs:70` (with the matching argument description at `:68`). So the exact-14-digit assertions in play are ddb's own — `DoogatId::is_valid_shape` and that schema wording — not a downstream break. The follow-up PRD scopes them as ddb-side updates; the genuine unknown is future downstreams, since ddb is positioned as a platform for consumers that do not exist yet.
- **Throughput**: no wall-clock throttle at the value-generation step — the ~1/sec ceiling in `generate_unique_id` exists only because the *candidate string* is second-granular; a sub-second suffix removes that constraint entirely. Real throughput becomes bound by however fast the process can compute a candidate and check it against the in-memory `HashSet` from `existence_oracle` — no disk I/O is structurally required (a purely in-process design need not persist the discriminator/counter across restarts the way `HlcClock` does). Estimate (not benchmarked in this repo): **on the order of 10^4-10^5 ID mints/sec/process**, several orders of magnitude above the ~1/sec baseline. End-to-end doogat creates do **not** follow it up: they stay at ~1/sec, bound by the per-doogat git commit (`docs/src/technical/performance.md:87`, `:95`).

## Option (c): HLC-derived identity

Shape: identity IS the HLC value (or a rendering of it), not a timestamp with an
HLC bolted on.

- **Filename stability**: mechanically fine for the same reason as (b) — `file_stem()`-based extraction (`ddb-core/src/parser/mod.rs:972`-`:977`) does not constrain length or character set. But the shape is a much bigger jump: `Hlc::to_string()` (`ddb-core/src/hlc.rs:110`-`:114`) renders `{wall_ms}-{counter:04}-{node}`, e.g. `1755000000000-0042-a1b2c3d4` — dashes and alphanumeric node characters, not pure digits, and no relation to the current `YYYYMMDDHHmmss` prefix convention. The 14-digit human-readable prefix is **not** retained under a literal HLC-string identity, unlike option (b)'s explicit retention. `DoogatId::is_valid_shape`'s "exactly 14 ASCII digits" invariant (id_minting.rs test comment, `:401`-`:409`) would have to be replaced outright, not relaxed.
- **Wikilink rewrite**: mechanically no forced rewrite of stored content — the wikilink regex (`ddb-core/src/parser/mod.rs:518`) still matches any non-`]`/`|` run, so an HLC-string target parses the same way syntactically. But *legibility* is lost: `[[20260226120000]]` reads as a date; `[[1755000000000-0042-a1b2c3d4]]` does not, which is a real (if not code-forcing) migration/UX cost noted here as the read-ability tradeoff for option (c).
- **Lexical-sort assumptions**: **breaks today** as written. `Hlc`'s own `Ord` (`ddb-core/src/hlc.rs:95`-`:102`) is correct (wall_ms, then counter, then node) but that is a *field-wise* comparison, not a *string* comparison. `Display`'s `wall_ms` field (`:112`) is **not zero-padded** — only `counter` is (`{:04}`). Two `Hlc` values whose `wall_ms` differ in digit count would sort correctly by `Ord` but out of order as raw filename/wikilink strings. (In the current epoch-millisecond range this is stable at 13 digits, so it happens to hold today, but it is not a structural guarantee the way zero-padded `YYYYMMDDHHmmss` is — a real gap the follow-up PRD would need to close, e.g. by fixed-width-encoding `wall_ms` before using it as an ID.)
- **Sync collision behavior**: collision-resistant by construction, on exactly the same terms as (b) and with exactly the same residual. `Hlc` already bakes in a per-node discriminator (`node`, truncated node UUID) as its lowest-priority tie-break (`ordering_node_tiebreak` test, `hlc.rs:228`-`:241`), and two nodes with distinct `.git/ddb-node` UUIDs mint distinct ephemeral or persisted node ids (`tick_succeeds_with_nonempty_node_when_ddb_node_absent`, `git_ops/hlc_clock.rs:412`-`:434`). But `Hlc.node` *is* `truncate_node`'s 8-character output, so (c) carries the identical 32-bit residual as (b) — it is not a stronger collision story, and `resolve_add_add_collision` remains the backstop under either option, not only for legacy collisions.
- **Downstream ID parsing**: breaks hardest *in principle* — any consumer that parses the ID as 14 digits, prefix or whole-string, fails outright, with no digit-only prefix to salvage. But the one downstream that exists was checked (see (b) above): jink parses nothing, so even (c) would not break jink's client code today. What (c) actually costs is ddb's own digit-based validators and schema wording, the date legibility of stored wikilinks, and every future consumer that assumes a date-like prefix. That is still the largest migration surface of the three; it is just not evidenced by a concrete jink break.
- **Throughput**: `Hlc::now` itself is a pure in-memory computation (`hlc.rs:18`-`:44`, no sleep, no I/O) — same as (b), no wall-clock throttle. But if identity minting reuses the existing persisted `HlcClock` (rather than inventing a separate non-persisted generator), every mint synchronously writes + renames `<git_dir>/ddb-hlc` (`git_ops/hlc_clock.rs:94`-`:103`) to stay durable across short-lived processes. That per-mint disk round-trip is the throughput ceiling in practice, not CPU. Estimate (not benchmarked): **on the order of 10^3-10^4 ID mints/sec/process** — still vastly above the ~1/sec baseline, but plausibly a full order of magnitude below a pure in-memory option (b) implementation that has no equivalent persisted-file write per mint. As with (b), end-to-end doogat creates stay at ~1/sec regardless, bound by the per-doogat git commit.

## Summary table

| Axis | (a) keep second-resolution | (b) sub-second suffix/counter | (c) HLC-derived identity |
|---|---|---|---|
| Filename stability | Compatible, unchanged | Compatible mechanically; `is_valid_shape`'s exact-14-digit invariant needs relaxing | Compatible mechanically; exact-14-digit invariant needs replacing (no digit-only shape) |
| Wikilink rewrite | Not applicable, unchanged | No forced rewrite; old 14-digit links keep resolving | No forced rewrite mechanically; legibility (date-like read) is lost |
| Lexical-sort assumptions | Holds (zero-padded 14-digit) | Holds, if suffix is fixed-width zero-padded (design constraint for the follow-up PRD) | Breaks today — `Hlc::to_string()`'s `wall_ms` field is not zero-padded; happens to hold at current epoch-ms digit count, not structurally guaranteed |
| Sync collision behavior | Reactive only — collisions resolved at merge via `resolve_add_add_collision` + `derive_content_id` | Collision-*resistant* at mint, if the suffix carries a real per-node discriminator (not a bare local counter); 2^-32 per-node-pair residual from `truncate_node`'s 32 bits, backstop still required | Same resistance and the *same* 2^-32 residual — `Hlc.node` is the same `truncate_node` output; backstop still required |
| Downstream ID parsing | Unaffected | Prefix-parsers keep working; exact-length validators need updating. jink verified opaque (`generated.ts:740`, `:1036`) — the only exact-14 assertions are ddb's own | Breaks all digit-based parsers; no salvageable digit-only prefix. jink still unaffected (parses nothing); cost falls on ddb's validators, wikilink legibility, and future consumers |
| Throughput (**ID mints**/sec/process) | **~1/sec** (current, measured) | Estimate: **~10^4-10^5/sec** (no wall-clock throttle, in-memory oracle lookup only) | Estimate: **~10^3-10^4/sec** (no wall-clock throttle, but bound by `HlcClock`'s per-tick file write+rename if the persisted clock is reused for identity) |
| End-to-end doogat creates/sec | **~1/sec** — git commit per doogat (`performance.md:87`, `:95`) | **~1/sec, unchanged** — the mint stops being the ceiling, the commit still is | **~1/sec, unchanged** — same commit bound |
