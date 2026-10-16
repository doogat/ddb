# Does ddb's architecture fit its purpose? (2026-07-12)

Purpose as stated by the maintainer: expose markdown files as a database through several
servers (GraphQL, REST, ...) so many clients on many platforms (mobile included) consume it
easily; distribution comes from git.

Method: two code audits (identity/merge path; server surface/concurrency) with file:line
evidence, plus `design-decisions.md`, `invariants.md`, and the PRD backlog (00163-00188).
Claims are marked verified (code read) or inferred (reasoning from verified mechanics).
Correction after first publication (same day): PgWire accepts writes (DML/DDL execute via
the actor); the first version of this doc wrongly said SELECT-only. Sections below fixed.

## Verdict

Three pillars, three grades:

1. **Markdown as a database** (parse, index, query): mostly sound. Git as truth with a
   rebuildable SQLite cache is the right call and honestly executed. One first-order hole:
   the served read path never re-checks truth, so edits made to the files themselves (the
   point of markdown) are invisible to clients (section 4).
2. **Many client surfaces**: over-delivered. All six transports exist and run today. They
   are also unequal (read-only NoSQL, FFI without warnings or sync, PgWire writes riding
   the SQL engine outside the app contract), so "120% parity" is currently an aspiration
   the shipped code contradicts (section 5).
3. **Distributed thanks to git**: the differentiator, and the weakest pillar. Confirmed in
   code: conflicted merges resurrect deleted files, conflict resolution is not deterministic
   across devices, HLC is absent from normal commits, IDs collide across devices by design,
   and mobile (FFI) has no git sync at all, only bundle export/import (section 3).

Net: the commodity layer (API breadth) got built six times while the moat (safe, convergent,
everywhere-including-mobile git sync) is half-built. At personal scale you will not fail on
throughput. You will fail on silent data wrongness and on mobile never joining the
distribution story. The fear is justified, but the failure mode is quieter than
"spectacular": resurrected notes, stale reads, links pointing at the wrong note, schemas
that differ per transport. For a product whose pitch is trust in your own files, quiet
wrongness is the worst kind.

The good news: the repo already knows most of this. The invariants doc exists, PRDs
00163-00167 sit at the head of the roadmap, the write lock shipped (00162), PgWire is
already SELECT-only, and server reads already bypass the write actor. Discipline is not the
problem. Allocation was.

## 1. What holds up (keep these)

- Git as sole truth; SQLite strictly derived and rebuildable (`design-decisions.md:42-48`).
  Index corruption is never data loss.
- Cross-process write lock on git writes, with e2e coverage (00162, shipped).
- ReadPool: GraphQL and PgWire SELECTs run on a semaphore pool of blocking readers, so a
  long sync/compact does not block them (`read_pool.rs:22-52`, `schema/queries.rs:62,210,301`).
- Batch writes commit once per batch (`git_ops/mod.rs:479-505`); SQLite updates are
  incremental, not full rebuilds, on normal writes.
- PgWire routes SELECTs to the ReadPool so they dodge the write actor
  (`pgwire.rs:102-107`). NoSQL API is GET-only and labeled experimental
  (`nosql_api.rs:27-32`, `ddb-cli/src/main.rs:439`); per the maintainer's 2026-07-13
  ruling that is a parity gap to close after the contract lands, not a specialization.
  PgWire is NOT read-only: non-SELECT SQL executes via the actor (section 5).
- Documented data-safety invariants with PRD tracking, and a roadmap that fronts safety
  (00163-00167). Rare hygiene. Protect it.

## 2. Identity: the ID scheme is wrong, and the system pays for it everywhere

Verified:

- IDs are `chrono::Local::now().format("%Y%m%d%H%M%S")`: wall-clock seconds, local
  timezone, no node component (`parser/mod.rs:939-940`; the HLC node id exists at
  `hlc.rs:13` but never enters the ID).
- Uniqueness is a process-local spin-wait plus, on some paths, a local-filesystem existence
  scan (`parser/mod.rs:933-947`, `service/write_helpers.rs:242-251`). `generate_id()` is
  literally `generate_unique_id(|_| false)` (`parser/mod.rs:927-929`), the exact pattern the
  AGENTS.md invariant forbids, and `create_from_content` uses it (`service/create.rs:429`).
  The invariant is violated in-tree today (00164's job).
- Two devices minting in the same second collide. Sync then runs a whole mitigation
  subsystem: an add-add bucket (`sync_manager/mod.rs:120-121`), an HLC tiebreak that
  usually has no HLC and defaults to theirs-wins (`mod.rs:133-136`), and loser reassignment
  that mints a fresh ID, rewrites frontmatter and links, and commits
  (`mod.rs:317-321,676-718`). That machinery's atomicity has its own PRD (00167).

Consequences:

- Both colliding documents survive (good), but which one keeps the ID depends on which
  device syncs first, and any wikilink written before sync can end up pointing at the other
  device's unrelated document after reassignment (mechanism verified; the dangling-link race
  is inferred, not traced live). "Filenames never change so wikilinks stay stable"
  (`design-decisions.md:34-40`) is exactly the promise reassignment breaks. The decision doc
  records only "opaque filenames" as the tradeoff; collision never appears.
- Mint rate is ~1 ID/second/process on the single-create path (spin until the clock leaves
  the minted second), so importing a 10k-note vault one create at a time spends ~3 hours
  minting. The SQL batch path avoids the wait by pre-claiming one FUTURE second per row
  (`sql_engine/mod.rs:127-168`, documented in PRD 00164), so a 10k-row INSERT reserves
  ~2.8 hours of future ID space and a later create landing in that window silently
  overwrites a batch row. First-run migration is the first thing a real user does.
- Local time, not UTC: cross-timezone devices interleave wrongly, and on the DST fall-back
  hour the exists-checked path can spin for up to an hour while the unchecked path can
  re-mint hour-old IDs (mint-loop behavior verified; downstream overwrite behavior
  unverified, flagged).

The point: hundreds of lines (collision buckets, reassignment, link rewriting, 00167) exist
only because the ID scheme collides. A UTC + node-discriminator ID deletes the failure class
for less than the cost of perfecting its mitigation. 00170 frames this as a throughput
decision; it is an identity-correctness decision.

## 3. The distribution pillar: four confirmed holes plus a missing platform

**a. Deleted files resurrect in conflicted merges (verified).** Merge tree construction
starts from the cached `repo.index()` and stages only Added|Modified, never Deleted
(`git_ops/merge.rs:22-43,117`). Delete a note on device A, have device B cause any unrelated
conflict, sync: the note returns, unmarked. Known (invariants I2, PRD 00163). Add to 00163:
merge should also build from `fresh_index()`, matching the shipped write-lock invariant.

**b. Conflict resolution is not deterministic across devices (verified mechanics).**
Automerge actors are never seeded (`AutoCommit::new()` at `crdt_resolver.rs:118,376,483`;
no `set_actor` anywhere), each device resolves with itself as "ours", and state is rebuilt
from blobs per conflict. True same-field conflicts tiebreak on a random actor id, so two
devices can resolve the same conflict differently; convergence relies on later git
propagation. Known (00165).

**c. HLC is wired but not load-bearing (verified).** Only merge commits and the singleton
sweep stamp the trailer (`sync_manager/mod.rs:347`, `consistency/singleton_sweep.rs:138,257`);
ordinary create/update commits never do. So "LWW by HLC" almost always falls back to
side-preference defaults (`crdt_resolver.rs:578-589`), and the default body strategy ignores
HLC entirely (`crdt_resolver.rs:773-774`). Known (00166). Three ordering mechanisms (git
3-way, automerge, HLC) each run partially; one made total beats three partial.

**d. Write-only CRDT state (verified).** `doc.save()` bytes land in `.crdt` temp files that
nothing ever loads back (the only `AutoCommit::load` is in a test,
`crdt_resolver.rs:937`); compaction exists partly to reap them. Either load the state
(stateful, deterministic CRDT) or stop writing it and shrink compaction.

**e. Mobile is not in the distribution (verified).** FFI `DoogatDriver` exposes no
fetch/push/sync; replication for a mobile app is bundle export/import only
(`ffi/driver.rs:240-268`). Core has `GitRemote::fetch/push` (`traits.rs:367-376`),
unbridged. "Distributed thanks to git, mobile included" is currently false on mobile, and no
backlog PRD adds it (00184 is a Rust client crate, 00185 a capability handshake). This needs
a model decision before code: mobile as full git replica (FFI sync, then face libgit2
credentials and iOS background limits) or mobile as thin client of a hosted server (then TLS
and real tokens stop being optional, section 6). Today it is neither.

## 4. "Markdown as a database" has a freshness hole

Verified: no file watcher exists workspace-wide; staleness detection is HEAD-oid comparison
only (`indexer/rebuild.rs:12-24`); server reads run with `skip_stale_check=true` and trust
the actor to have rebuilt (`read_pool.rs`, `service/mod.rs:110-124`).

Consequences while the server runs: an uncommitted editor edit is invisible to every
transport indefinitely; even a committed edit stays invisible to GraphQL/PgWire until some
actor operation flips HEAD and triggers a rebuild. Clients get no signal. For a product
whose premise is that the files ARE the database, served surfaces not noticing file edits is
a first-order gap, and nothing in the backlog covers it. Minimum fix: a cheap staleness
probe on read (one HEAD read) or a watcher feeding the reload path, plus a written
consistency contract stating what freshness each transport promises. Related known limit:
subscription events drop silently for slow clients (broadcast capacity 256,
`design-decisions.md:105-111`).

## 5. The six-transport doctrine vs. the six shipped transports

The positioning (`design-decisions.md:5-16`) makes breadth deliberate and forbids reading
"one consumer, one transport" as waste. Assessed within that positioning, the shipped
reality still argues for consolidation:

- Parity is already broken between shipped transports. FFI create/update bypass the app
  contract and return no warnings (`ffi/driver.rs:68,84` vs `actor/handlers.rs:61,81`;
  known, 00176). NoSQL is read-only GETs. PgWire accepts full DML/DDL: non-SELECT
  statements route to the actor's SQL engine and execute, DDL triggering schema reload
  (`pgwire.rs:102-121`), so PG-protocol clients get write access with none of Postgres's
  write semantics (no ROLLBACK once committed, no isolation levels, and the SQL-engine
  error arm is the one missing from unified error mapping, 00172/00179). A `_typedef` arriving via
  git sync updates SQL tables but NOT the GraphQL schema (the Sync handler rebuilds only the
  index, `handlers.rs:155-161`; reload triggers exist only in applySchema and PgWire DDL),
  and open subscriptions keep their pre-upgrade schema until reconnect (`ws.rs:42`).
  NoSQL reads route through the write actor and stall behind sync/compact; GraphQL reads do
  not (`nosql_api.rs` to `handlers.rs:11-16` vs `read_pool.rs`). 00183/00185 touch schema
  evolution but neither clearly states sync-triggered GraphQL reload; verify when picked up.
- AGENTS.md's own rule: CRUD is the Guaranteed baseline for every public interface unless
  explicitly declared specialized. NoSQL currently fails that (GET-only). The maintainer
  declined the exception clause (2026-07-13): all six interfaces stay Guaranteed full-CRUD,
  so GET-only NoSQL is a gap to close once the contract makes adding verbs thin.
- The redb mirror is a fourth persistence layer (git, SQLite, redb, `.crdt` temp),
  dual-written on every CRUD, reopened per call around redb's exclusive lock
  (`nosql.rs:225-247`), best-effort by contract (`traits.rs:329-342`), serving three GET
  routes SQLite already answers. A parity artifact, not a capability.

This doc originally recommended shrinking the Guaranteed surface (specialize PgWire and
NoSQL as read veneers). The maintainer overruled that (2026-07-13): all six interfaces
stay Guaranteed for full CRUD, and the remedy is attacking the root cause instead. That
root cause is real and is the positioning's own load-bearing promise: "app contract keeps
parity cost sub-linear" is the half-built part (00171-00177). Until it lands, every
capability costs six hand-plumbed implementations, and drift like the schema-reload split,
the FFI warning gap, and GET-only NoSQL is the observable result. The overrule sharpens
the priority: finishing the contract is not one workstream among several, it is the
precondition for the doctrine being affordable at all.

## 6. Production posture (the moment any client leaves localhost)

- One static bearer token for everything, plain `==` compare (`auth.rs:30-34`), reused as
  the PgWire md5 password (`pgwire.rs:69-73`). No TLS anywhere in the server (no
  rustls/native-tls dep); default bind 127.0.0.1. Fine for localhost, disqualifying for a
  network-exposed backend. 00179 exists but sits mid-roadmap.
- Omitted `limit` means unbounded: `validate_limit` returns None and the pool emits
  `LIMIT -1` (`schema/base_types.rs:1067-1080`, `read_pool.rs:148-150`). No rate limiting,
  no request timeout layer; body cap only by framework default (inferred, not configured).
- Scale: the design doc itself caps MVP at <5K doogats (`design-decisions.md:48`); benches
  run at 1K. "Database" claims beyond that are unvalidated.

## 7. Failure scenarios, ranked by trust damage

1. Deleted note returns after a conflicted sync (code-confirmed, 3a).
2. Wikilink silently points at a different document after collision reassignment
   (mechanism confirmed, race inferred, section 2).
3. Two devices disagree after resolving the same conflict (mechanics confirmed, 3b/3c).
4. Client apps serve stale data after direct file edits, with no signal (code-confirmed,
   section 4).
5. Mobile app never syncs; users shuttle bundle files (feature absent, 3e).
6. Vault import takes hours; the DST fall-back hour stalls or double-mints
   (construction-confirmed; edge behavior unverified, section 2).
7. Exposed server: sniffable static token, unbounded queries (code-confirmed, section 6).

## 8. What I would change, in order

1. **Land 00163-00167 before anything else** (already the roadmap head; protect it). Fold
   into 00163: merge tree construction from `fresh_index()`, deletions staged.
2. **Reframe 00164+00170 as identity correctness, not throughput**: UTC mint plus a node
   discriminator (keep the 14-digit prefix for humans; a suffix removes cross-device
   collision), one mint path with real existence checks, then retire the
   collision-reassignment machinery over time.
3. **Pick one ordering spine** (treat 00165/00166 as a pair): either HLC stamped on every
   commit with CRDT actors derived from stable inputs, or drop automerge for deterministic
   field-LWW plus line-union body merge. Stop writing never-read `.crdt` state either way.
4. **Decide the mobile model** (new PRD, decision before code): FFI git sync (full replica)
   or hosted server with TLS and per-client tokens (thin client). The stated purpose is
   unmet until one of them exists.
5. **Close the freshness hole** (new PRD): read-path staleness probe or file watcher, plus a
   one-page consistency contract per transport (include schema-reload-on-sync).
6. **Close the NoSQL parity gap after the contract lands** (new PRD): full CRUD routes on
   `/nosql` written as thin adapters over the app contract. Treat "one screenful of adapter
   code per verb" as the acceptance test that 00171-00177 actually fixed the
   six-implementations problem; if NoSQL CRUD needs more, the contract is still half-built.
   Decide inside that PRD whether NoSQL reads stay on the redb mirror or move to the core
   index: a per-transport private read store is the same divergence disease the contract
   rule ("transports serialize the core type") exists to prevent, and the mirror is
   best-effort by contract (`traits.rs:329-342`), which sits awkwardly under a Guaranteed
   interface. PgWire DML/DDL folds into 00172/00179 per the maintainer decision below.

## Maintainer decisions (2026-07-13, from the critique discussion)

- **Identity reframe: agreed.** 00164/00170 are identity-correctness work (UTC mint plus
  node discriminator), not a throughput decision.
- **PgWire: keep writes, contract them.** Fold PgWire DML/DDL into the 00172/00179
  contract and error-table work; document the non-transactional semantics (no rollback
  after commit, no isolation levels) in the interface contract. Supersedes this doc's
  freeze recommendation.
- **No specialization exceptions, for any interface.** All six interfaces stay Guaranteed
  for the full CRUD set. The six-implementations cost is the real problem and gets attacked
  at the root (finish the app contract, 00171-00177), not avoided by shrinking the surface.
  GET-only NoSQL is therefore a parity gap: new PRD, sequenced after the contract block.
- **Mobile model: FFI git sync, full replica.** Bridge fetch/push/sync into DoogatDriver
  (HTTPS-token remotes first); bundles remain the offline fallback; hosted thin-client
  access can layer on later via 00179. New PRD, sequenced after the safety and contract
  blocks.
- **Freshness: probe + watch mode.** Server reads get a cheap HEAD-oid staleness probe
  (committed = visible becomes enforced, not accidental), plus an opt-in watcher that
  auto-commits external edits after a debounce, plus a written per-transport consistency
  contract and sync-triggered GraphQL schema reload. One PRD.

No items from this critique remain undecided.
7. Then the contract work (00171-00177) carries the remaining parity honestly; 00179
   hardening and default query limits land before any network exposure.

## 9. Bottom line

Architected well? The storage core: yes. The transport layer: competently built, prematurely
multiplied, currently claiming a parity the code does not have. The distribution story, the
reason this product should exist: not yet sound, and absent on mobile. It will not collapse
with fireworks; it will leak trust through silent anomalies unless the roadmap head lands
first and identity, freshness, and the mobile decision are added to it. Your instinct is
right in direction, wrong in shape: fear quiet corruption, not spectacle.
