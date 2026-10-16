# Backlog review - ddb - 2026-09-05

Verdict: **GO — 28 backlog PRDs READY; 0 Blocking open, no waivers, no pending decisions or reshapes.**

Initial review at HEAD **5154234ba3bf48a59bc5d921b8b2391e510a6134** read all 29 then-backlog PRDs end-to-end and ran all eight lenses. The apply pass updated the 28 remaining backlog PRDs after delegated engineering decisions and the maintainer's beta-compatibility clarification. This review changed no product code, wip/done/hold contents, autopilot state, or gates.

**Concurrent-work resolution:** 00202 moved to done in the other session. Commits **52759bd** (retained-parent regressions) and **a7e5179** (matching-parent traversal) address B01; current read.rs follows matching parents breadth-first in parent order and tests cover retained first/second parents and multiple matches. This review inspected that evidence without modifying the implementation or completed PRD, and did not rerun its Rust tests.

The earlier review and its accepted decisions are preserved verbatim in [backlog-review-2026-09-05-earlier.md](backlog-review-2026-09-05-earlier.md). Those choices remain authoritative: per-code HTTP statuses, FFI BaseOnly, null omit/unset behavior, the server's numerical bounds, the relation-filter scope exclusions, capability seed, redb retention, existed naming, CI-only mobile artifact builds, the grouped 00183 sweep, updated_at commit-time semantics, and the explicit 00202-before-00171 operator sequence. The maintainer subsequently clarified that this is a solo-developer beta with no production clients: direct API replacements are authorized, with migrated in-repo callers and migration guidance, without legacy entrypoints or a deprecation window.

## Map

Paths in the PRD column are relative to dev/local/prds/backlog/. Dependencies include inferred producer/consumer edges; completed foundations are marked done. READY means the revised artifact has no open Blocking finding, subject to dependencies. Finish the existing 00171 run before draining this backlog; its shell-harness migration is still in progress.

| # | PRD | template | lines | subsystems | depends on | verdict |
|---|-----|----------|-------|------------|------------|---------|
| 00172 | 00172-collapse-actor-plumbing-v1.md | standard | 158 | actor, reload, server adapters | none | READY |
| 00173 | 00173-unify-server-error-mapping-paths-v1.md | standard | 168 | core error contract, server renderers | none | READY |
| 00174 | 00174-decouple-maintenance-from-write-actor-v1.md | standard | 177 | maintenance worker, git remote/lock | 00162 done, 00172 | READY |
| 00175 | 00175-route-read-delete-through-contract-v1.md | standard | 149 | read/delete, four adapters, REST test driver | 00172 | READY |
| 00176 | 00176-route-search-through-contract-v1.md | standard | 164 | search contract, pagination, four adapters | 00172, 00175 REST driver | READY |
| 00177 | 00177-ffi-crud-through-contract-v1.md | standard | 148 | FFI CRUD, error policy | 00172/00173/00175 | READY |
| 00178 | 00178-structured-fields-input-v1.md | standard | 148 | GraphQL/REST/FFI inputs | 00172/00175/00177 | READY |
| 00179 | 00179-sql-identifier-escaping-materializer-v1.md | standard | 194 | SQL identifiers, validation, materialization | none | READY |
| 00180 | 00180-server-security-resource-hardening-v1.md | standard | 197 | server limits/auth, PgWire, NoSQL | 00173/00176 | READY |
| 00181 | 00181-index-health-quarantine-surface-v1.md | standard | 140 | read-only health scan, adapters | 00169 done | READY |
| 00182 | 00182-where-filter-correctness-completeness-v2.md | standard | 164 | relation filters, LIKE, junction indexes | 00179 quoting moves precede it | READY |
| 00183 | 00183-split-oversized-modules-round-2-v1.md | standard | 183 | core/server module splits | earlier contract/filter edits | READY |
| 00184 | 00184-schema-apply-v2-index-evolution-ffi-zone-parity-v1.md | standard | 154 | schema differ/apply, FFI discovery | 00161 done, 00175 REST driver, 00183 moves | READY |
| 00185 | 00185-ddb-client-rust-crate-v1.md | standard | 185 | new client crate, parsers, auth | 00173 code vocabulary | READY |
| 00186 | 00186-offline-sdl-export-and-capability-handshake-v1.md | standard | 180 | schema export, capability registry, CLI | existing schema builder, 00183 moves | READY |
| 00187 | 00187-idempotent-upsert-return-existing-v1.md | standard | 181 | core conflict resolution, create adapters | 00177; 00178 soft; 00186 registry | READY |
| 00188 | 00188-ephemeral-serve-test-mode-v1.md | standard | 174 | CLI/server startup, ports, shutdown | existing server; 00186 registry | READY |
| 00189 | 00189-rust-typedef-codegen-v1.md | standard | 181 | generated Rust, GraphQL naming/schema | 00186, 00183 moves | READY |
| 00190 | 00190-close-read-freshness-hole-v1.md | standard | 198 | read pool, watcher, schema reload | 00169 done, 00172/00174; 00186 registry | READY |
| 00191 | 00191-ffi-git-sync-mobile-replica-v1.md | standard | 187 | core remote/sync, FFI, credentials | 00163/00165/00167 done, 00174/00177; 00186 registry | READY |
| 00192 | 00192-nosql-http-full-crud-parity-v1.md | standard | 173 | NoSQL adapters/mirror, conformance | 00172-00178, 00187 existed, 00190 consistency doc; 00186 registry | READY |
| 00193 | 00193-pgwire-ffi-conformance-legs-v1.md | standard | 184 | FFI/PgWire drivers, disposition registry | 00192, earlier contract adapters | READY |
| 00194 | 00194-ffi-bindings-ci-v1.md | minimal | 66 | nightly bindings/platform CI | 00191 sequencing | READY |
| 00195 | 00195-guard-batch-rmw-lost-updates-v1.md | standard | 142 | git guarded commits, migrations, quarantine | 00162 done, 00183 moves | READY |
| 00196 | 00196-route-sql-dml-through-write-pipeline-v1.md | standard | 139 | shared service/SQL write plan | 00175-00178, 00183 | READY |
| 00197 | 00197-derive-transport-renders-from-core-types-v1.md | standard | 177 | report render bridge, canonical SQL results | 00172/00176, 00183 moves, 00193 drivers; 00186 registry | READY |
| 00198 | 00198-verify-zone-default-null-write-v1.md | standard | 129 | reproduction matrix, conformance | 00178 null contract, 00193 drivers | READY |
| 00199 | 00199-prune-dead-abstractions-and-hygiene-v1.md | minimal | 84 | migration gate, dead traits, config, timestamps | 00183 moves | READY |

Final inventory: **28 backlog files, 4524 lines, 164 tasks; wip 1 (00171), hold 3, done 18**. The only lifecycle change during the review was the other session completing 00202. No stray backlog files, duplicate numeric prefixes, or competing 00182 versions were found; the absorbed 00169b hold artifact is not executable backlog work.

All 28 revised PRDs match their chosen template heading skeleton and ordering. Every task has an Acceptance clause, every Capability has grouped Module coverage, Feature headings are unique, and no template stubs remain. Recognized frontmatter values are valid, including the live create-prd model_tier_rationale field.

Initial 29-file input digest: **30a029840d7527735b5c37146666d547069e0fbbd2d223caa995c070578c02ba**. Final 28-file digest (sorted filename, NUL, raw bytes, NUL): **4c03bb24da76f528857b7dd10e0316096250194900e9863febc159cbbd71e08d**. Edits were applied as contextual patches; concurrent product changes and 00202's lifecycle move were preserved.

## Findings

### Blocking

The findings below preserve the original evidence and pre-edit locations; they are historical findings, not open defects in the revised PRDs. B01 concerns 00202, now in done. The Decisions applied table records the resolution of every Blocking finding, including where the maintainer selected a different compatibility policy.

#### B01 — 00202: the HLC walk can choose a discarded parent lineage

- **Lenses:** B/C/D. **Location:** 00202, Feature “Touch predicate ignores files a merge merely transported”, lines 37, 58, 105.
- **Evidence:** The PRD changes the touch predicate but keeps find_hlc_for_path unchanged. ddb-core/src/git_ops/read.rs:84-96 walks the entire ancestor DAG and returns the first touching commit. Counterexample: A writes x=old at HLC 1000; descendant B writes x=new at HLC 2000; an ours-strategy merge with parents [A,B] retains x=old. The proposed predicate skips the merge because it equals A, but topological order must visit B before A and returns HLC 2000 for discarded content.
- **Fails as:** wrong-TDD lock-in. Both prescribed tests can pass while the promised authoring-commit attribution remains false.
- **Recommended edit:** allow traversal changes and require transparent merges to follow parent lineage(s) matching the selected path entry. Add retained-parent/rejected-parent regression coverage, plus deterministic handling when several parents match. Preserve root/single-parent and genuine merge-rewrite behavior. Remove “find_hlc_for_path unchanged” and the one-predicate reason for design: skip; use the design phase to settle traversal. Also change the literal assertion check at line 113 to **rg -F -c**: ordinary rg -c treats parentheses as regex syntax and returns no match for the specified line.

#### B02 — 00195: independent file skipping breaks compound operations and migration retries

- **Lenses:** B/C/D. **Location:** 00195, Success Metrics line 33; guarded-helper Features lines 42-51; exported signature line 73; adoption task line 108; all-stale scenario.
- **Evidence:** The new helper accepts writes and present OIDs only. singleton_sweep actually writes a new quarantine path and deletes the source together (ddb-core/src/consistency/singleton_sweep.rs:102-107,143). migrate_all commits .ddb/migration-version with the file rewrites (consistency/migrations.rs:152-169), then future runs omit completed versions (:88-96). Dropping a stale file while advancing that marker contradicts “a re-run picks it up.” The helper also promises a mandatory CommitHash while the all-stale scenario promises no commit.
- **Fails as:** wrong-TDD lock-in, potentially deleting a concurrent edit or permanently leaving a file unmigrated.
- **Recommended edit:** define guarded logical units, including writes, deletions, expected absence, and OIDs captured from the same snapshot as the source content. Skip a quarantine move's source/deletion/destination together. If a migration cannot complete, defer that migration unit without advancing its marker; pin a successful retry after the competing writer stops. Represent no-commit explicitly and report only applied changes plus structured skipped-unit warnings. Add quarantine-race, migration-rerun, and all-stale tests.

#### B03 — 00173/00177: the shared error policy is placed above its core FFI consumer

- **Lenses:** C/D/E/G/H. **Location:** 00173, Repository Structure and Module: error, lines 87-90; 00177, policy-derived FFI Feature and task, lines 51-54,108.
- **Evidence:** 00173 puts ErrorRendering and the sole policy in ddb-server/src/error.rs; 00177 requires ddb-core FFI to consume it. ddb-server/Cargo.toml:10 already depends on ddb-core. A reverse dependency is a Cargo cycle; a second FFI policy violates the one-table invariant.
- **Fails as:** rework thrash.
- **Recommended edit:** put the adapter-neutral policy table and rendering data in ddb-core's app contract, including code/category/status/redaction/FFI hint. Use neutral representations rather than HTTP or UniFFI adapter types. The existing core category conversion and all transport encoders derive from that table. Update both PRDs' ownership/dependency descriptions. Preserve the already-decided per-code statuses, and replace the residual “status per category” acceptance/transport wording in 00173.

#### B04 — 00174: transfer callbacks cannot enforce the promised remote deadline

- **Lenses:** B/D. **Location:** 00174, Remote timeout Feature line 63 and Phase 0 task line 125.
- **Evidence:** Installed git2 0.21.0 exposes fetch transfer progress as a boolean callback, but push transfer progress returns unit (src/remote_callbacks.rs:155-159,215-219). Progress callbacks do not execute while a connection is silently stalled before progress; the push callback has no cancellation return.
- **Fails as:** unattended hang / rework thrash. The specified never-completing-transfer test cannot pass using the prescribed mechanism alone.
- **Recommended edit:** retain the 30-second configurable deadline and one options/credentials construction point, but remove the callback-only prescription. Require an abort-capable implementation covering pre-progress and mid-transfer stalls on fetch and push, with no remote operation continuing after timeout. Retain write-lock protection for actual repo writes without holding the repo write lease over the blocked network interval.

#### B05 — 00176: the search result is both a page and a list of pages

- **Lenses:** B/C. **Location:** 00176, canonical-hit Feature line 45; structural mapping lines 70,83; Phase 0 task line 110.
- **Evidence:** The Feature redefines AppSearchResult as a container of ten-field hits, but the service task demands AppOutput<Vec<AppSearchResult>> and describes each AppSearchResult as a ten-field hit. The existing type is a page-like result with items, total, query (ddb-core/src/app_contract/results.rs:57-61).
- **Fails as:** wrong-TDD lock-in. A test author can encode either nested pages or hits and satisfy one part while violating the other.
- **Recommended edit:** keep AppSearchResult as the page containing items: Vec<the canonical core SearchResult>, total, and query; return AppOutput<AppSearchResult>. Name the hit and page consistently in Features, tree, exports, tasks, and conformance assertions; preserve pagination metadata.

#### B06 — 00176/00180: the shared pagination policy has no default or maximum

- **Lenses:** B/C/E. **Location:** 00176, PageRequest Feature lines 48-52, task line 110, Risks line 147; 00180's NoSQL cap Feature/task.
- **Evidence:** The PRD promises “a concrete default” and “a max” without values, while saying the absent-limit default is unchanged. Existing REST uses 50/200 (ddb-server/src/rest.rs:253-278); GraphQL accepts up to 10,000 and can default to unbounded (schema/base_types.rs:1030-1080).
- **Fails as:** wrong-TDD lock-in. 00180 consumes a numerical policy its producer never fixes.
- **Recommended edit for decision:** pin default limit 50, maximum 10,000, offset default 0, positive limits and non-negative offsets; preserve the accepted over-max InvalidInput rejection. Document zero/negative/overflow behavior and each adapter's page-to-offset conversion, and pin boundary tests. Record the GraphQL/REST compatibility changes rather than claiming every old default is unchanged.

#### B07 — 00175: replacement response types are described as additive

- **Lenses:** B/C/D. **Location:** 00175, Success Metrics line 29; Risks lines 128-129; Phase 1 adapter tasks.
- **Evidence:** Current FFI read/delete return String/unit (ddb-core/src/ffi/driver.rs:73,89); GraphQL delete returns Boolean (schema/mutations/operations.rs:179-188); REST delete returns 204 (rest.rs:441-447). Replacing those with records/envelopes is not adding a warnings field to an existing record.
- **Fails as:** wrong-TDD lock-in / rework thrash. “Existing tests stay green” and an additive-only migration conflict with the prescribed new wire types.
- **Recommended edit for decision:** specify a compatibility window with additive warning-bearing entrypoints and explicitly deprecated legacy ones, naming the supported old/new requests and results. Alternatively explicitly approve the breaking replacements and their release/migration boundary. In either case add legacy-client and new warning-bearing contract tests; do not describe the replacement as additive.

#### B08 — 00178: dual decoding does not preserve legacy GraphQL String variables

- **Lenses:** B/C/D. **Location:** 00178, legacy-form success metric, Repository Structure line 54, Phase 1 task line 96.
- **Evidence:** The existing GraphQL input is fields: String (schema/type_defs.rs:437,448,466). Changing it to JSON invalidates an existing operation declaring $fields: String before the object-or-string decoder runs. Installed async-graphql 7.2.1 enforces named scalar compatibility in validation/rules/variables_in_allowed_position.rs:41-59.
- **Fails as:** wrong-TDD lock-in. Tests using string literals or JSON variables miss broken existing clients.
- **Recommended edit for decision:** retain the legacy String-typed input during the window and add an object-capable input, with defined behavior when both are supplied. Test a legacy $fields: String operation as well as native boolean/number object input. A breaking scalar migration is the alternative and must be stated explicitly.

#### B09 — 00177/00178: the FFI fields representation and null/unset result are not defined

- **Lenses:** B/C/D/E. **Location:** 00177, structured create/update inputs and Phase 0 records; 00178, object decode Feature, core-unchanged claim, Module: fields decode, Phase 0/1 tasks.
- **Evidence:** ddb-core/src/types/value.rs:5-12 has String/Number/Bool/List/Map, no Null and no UniFFI conversion. UniFFI 0.29.5 supplies map lifting for HashMap, not the specified direct BTreeMap surface. The one decoder is assigned to ddb-server, yet FFI is inside ddb-core. A map-only output cannot express the accepted difference between create omission and update unset.
- **Fails as:** wrong-TDD lock-in / rework thrash.
- **Recommended edit:** specify a UniFFI-safe adapter input representation and its conversion into the existing core map; require native scalar fidelity. Put shared business normalization in the app contract, with transport decoding at each boundary. Preserve the decided null rule by producing both the field map and update unset_fields, not a nonexistent Value::Null. Cover FFI generation, typed scalars, omitted create keys, and update unsets. Let design settle the private representation without requiring a core-to-server dependency.

#### B10 — 00179: promised helper/validator consolidation has no implementation task

- **Lenses:** A/C/G. **Location:** 00179, Feature “Shared quote_ident util in ddb-core”; Implementation Phases, especially Phase 0.
- **Evidence:** The Feature requires removing escape_sql_ident and converting its 17 manually wrapped callers, plus consolidating is_safe_sql_identifier and validate_identifier. The tasks promote quote_ident and patch other sinks but never assign those two consolidation legs. Repository Structure also omits indexer/filter.rs, schema_diff/desired.rs, and the relevant search callers.
- **Fails as:** rework thrash. All written tasks can finish while the single-helper/single-validator feature fails final review.
- **Recommended edit:** add explicit consolidation tasks and file/module coverage, with existing-call-site checks, no double quoting, and preserved safe-name behavior. State how the canonical validator preserves the create boundary's accepted safe-but-unknown names while serving the stricter typedef grammar; do not silently broaden or narrow a write contract.

#### B11 — 00180: DefaultBodyLimit does not limit this GraphQL extractor

- **Lenses:** B/D. **Location:** 00180, Router-wide body limit Feature line 66 and Phase 0 task line 141.
- **Evidence:** async-graphql-axum 7.2.1 consumes req.into_body().into_data_stream() directly (installed src/extract.rs:122-130). axum-core 0.5.6's DefaultBodyLimit documentation explicitly excludes direct body consumption (extract/default_body_limit.rs:12-24). REST Json also retains an implicit smaller limit unless adjusted.
- **Fails as:** wrong-TDD lock-in / rework thrash. The prescribed layer can pass REST tests while GraphQL's required 8 MiB+1 → 413 fails.
- **Recommended edit:** require a global RequestBodyLimitLayer or equivalent bounded reader before extraction, and align Json's extractor limit to the accepted 8 MiB value. Preserve 413 at the boundary. Test exact/over-limit bodies through REST, GraphQL, and NoSQL, including requests without Content-Length.

#### B12 — 00180: the selected comparison primitive is not length-independent

- **Lenses:** B/D. **Location:** 00180, Success Metrics line 35; Constant-time token compare Feature lines 72-73; Phase 1 task line 151.
- **Evidence:** subtle 2.6.1's slice ConstantTimeEq short-circuits unequal lengths and iterates equal-length slices (installed src/lib.rs:294-320). An ordinary timing unit test cannot establish a constant-time guarantee.
- **Fails as:** wrong-TDD lock-in / rework thrash.
- **Recommended edit for decision:** specify content-independent comparison for equal-length byte slices, with explicit length rejection; pin correct/incorrect/equal-prefix/different-length functional cases and use of the audited primitive. If token-length hiding is required, specify a fixed-size digest comparison instead. Keep the chosen security property explicit.

#### B13 — 00185: porting the reference parsers ports their silent data loss

- **Lenses:** D/H. **Location:** 00185, parser Feature line 59 and Phase 0 task line 132.
- **Evidence:** jink/src/ddb_client.rs:188-196 filter_map-drops malformed connection items, while :214-225 replaces malformed SQL rows with empty vectors; its test at :350 pins skipped items. Copying that behavior violates ddb's errors-or-structured-warnings rule for row/parse failures.
- **Fails as:** wrong-TDD lock-in / goal reversal.
- **Recommended edit:** require a typed parse error for malformed connection items and SQL rows, or an explicit structured per-item warning result; add both regression cases to task acceptance. Preserve the already-settled policy for a GraphQL envelope containing non-null data plus errors; envelope handling and item parsing are separate decisions.

#### B14 — 00186: a waived citation is still a mandatory nonexistent input

- **Lenses:** B/D. **Location:** 00186, Phase 0 task line 125 and problem citation line 16.
- **Evidence:** The task mandates reading jink/dev/local/prds/wip/00026-adopt-ddb-completeness-enhancements-v1.md. It currently exists in jink's backlog directory, not wip. The relevant probes cover orderBy, relation/negation filtering, and inline ZONE. The link-ok token only waives the mechanical checker.
- **Fails as:** stall at the mandatory premise/source check.
- **Recommended edit:** embed those three probe requirements with provenance in this PRD, retaining the accepted six-flag seed; any fresh cross-repo corroboration is optional. Alternatively resolve the unique 00026 PRD across lifecycle directories with a defined missing/duplicate-source outcome. Do not replace one brittle lifecycle path with another.

#### B15 — 00187: ordinary unique violations do not supply existing_id

- **Lenses:** B/D. **Location:** 00187, return-existing Features lines 44,48 and Risks line 153.
- **Evidence:** ddb-core/src/error.rs:125-149 constructs UNIQUE_VIOLATION context from table/columns/values, not existing_id; sql_engine/dml.rs:1532-1538 uses it. Singleton violations do carry an existing id. service/create.rs:580 discards the matched identity when building the ordinary unique error.
- **Fails as:** wrong-TDD lock-in / rework thrash. The required race-safe catch-after-conflict path consumes an identity its producer never supplies.
- **Recommended edit:** explicitly assign core conflict-identity resolution from actual constraint metadata inside the transactional path. Pin composite and multiple constraints plus a conflict appearing after pre-check, with an unambiguous existing row or a defined ambiguity error. No new public error field is required merely to supply the internal resolver.

#### B16 — 00189: generated types can compile while failing valid GraphQL responses

- **Lenses:** B/D. **Location:** 00189, type/input mapping Features lines 37-44,63-69 and Phase 2 validation task line 140.
- **Evidence:** schema/base_types.rs:894-900 emits nullable GraphQL scalars even for required columns. REFERENCES expands into object/list/raw-ID fields (:807-846), not one scalar selected by a sanitized column name. Compile checks and matching SDL names do not prove a selection is valid or a response deserializes.
- **Fails as:** wrong-TDD lock-in.
- **Recommended edit:** distinguish read-wire nullability from create/update requirements, and pin REFERENCES selection/field expansion against the served schema. Require executing a generated selection and deserializing the actual response for fixtures containing a required scalar and a reference. Preserve shared naming and deterministic output; do not infer read nullability solely from ColumnDef.required.

#### B17 — 00190: stale-success fallback contradicts committed-equals-visible

- **Lenses:** B/C/H. **Location:** 00190, maintainer decision line 22; probe Feature line 43; consistency-doc Feature line 76; Phase 0 task line 141.
- **Evidence:** The output must be at least as fresh as HEAD at probe time, but the timeout path returns the old index successfully with only a server log. The contract page then claims committed = visible without an exception. A downstream cannot distinguish the stale response.
- **Fails as:** wrong-TDD lock-in / goal reversal.
- **Recommended edit:** keep the accepted freshness guarantee and five-second bound; on failure to obtain the required snapshot, return a structured retryable failure rather than a successful stale result. Pin the timeout response on the served transports and document retry behavior. An opt-in, visibly marked stale-read mode would require an explicit contract decision.

#### B18 — 00191: fetch/push report producers do not exist

- **Lenses:** B/D/G. **Location:** 00191, Fetch-only and push-only Feature lines 54-58 and Phase 1 task line 135.
- **Evidence:** GitRemote::fetch/push return Result<()> (traits.rs:388-391; git_ops/remote.rs:12-23). SyncManager has only full sync, whose direction/count fields are constructed from merge outcomes (sync_manager/mod.rs:283-308). 00174 adds timeout/options scaffolding, not report-producing halves.
- **Fails as:** wrong-TDD lock-in. A nominally thin FFI bridge must fabricate the report and cannot substantiate zero-transfers-as-no-op.
- **Recommended edit:** add explicit core fetch/push result production before FFI adaptation; define half-operation direction/count/no-op semantics and test them at core level. Retain the accepted lossless seven-field SyncReportRecord mirror. The FFI adapters serialize core outcomes without inventing counters.

#### B19 — 00191: a push failure cannot promise an untouched local repo

- **Lenses:** B/C/D. **Location:** 00191, mid-sync credential expiry behavior line 50 and Error case line 154.
- **Evidence:** service/concrete_index.rs:22-34 can register a node before fetch. sync_manager/mod.rs:263-271 applies the merge before finalization; sync-state updates and singleton resolution precede push at :422. A rejected push can follow completed local commits.
- **Fails as:** wrong-TDD lock-in / rework thrash. Reused sync and “repo state untouched” require incompatible behavior.
- **Recommended edit:** specify stage-specific recovery: failures before merge create no merge; a post-merge push failure preserves completed local changes, returns a typed error, and supports a safe retry. Include bootstrap side effects in the description. Pin a push rejection after successful fetch/merge, with no rollback of valid local data.

#### B20 — 00192: DELETE 204 conflicts with the shared warning-bearing result

- **Lenses:** B/C/E. **Location:** 00192, DELETE Feature line 48 and CRUD scenarios lines 146-148; producer 00175, delete Feature and adapter acceptance.
- **Evidence:** 00192 pins a bodyless 204 while requiring error/warning parity with REST and AppOutput. 00175 requires DeleteResult plus BrokenBacklink warnings; core already defines backlinks on DeleteResult (app_contract/results.rs:19-21).
- **Fails as:** wrong-TDD lock-in.
- **Recommended edit:** make new NoSQL DELETE return the warning-bearing contract result with a body-capable success status, aligned with the 00175 decision. Add delete-with-broken-backlink parity. Preserving 204 requires an explicitly specified lossless warning carrier; it cannot silently discard warnings.

#### B21 — 00193: the FFI conformance driver selects deprecated raw CRUD

- **Lenses:** D/E. **Location:** 00193, FfiDriver Feature line 55 and driver task line 139; producer 00177, lines 107,128.
- **Evidence:** 00193 names create_doogat/update_doogat and claims 00177 routes them through the contract. 00177 explicitly keeps those methods as raw legacy methods and introduces create_structured/update_structured.
- **Fails as:** wrong-TDD lock-in / rework thrash. The conformance driver would test the deprecated validation policy and absent warnings.
- **Recommended edit:** name create_structured/update_structured plus 00175's warning-bearing read/delete methods and records in the Feature and acceptance. Keep legacy behavior tests separate from the six-interface contract comparison.

#### B22 — 00194: all five tasks lack the required Acceptance clause

- **Lenses:** A/B. **Location:** 00194, Tasks lines 48-54.
- **Evidence:** Each task ends with prose after a hyphen, but none labels the verbatim Acceptance clause required by the review skill and installed plan-tasks step 4.
- **Fails as:** wrong-TDD lock-in: the planner must invent the boundary/content it copies into test-author context.
- **Recommended edit:** add “Acceptance:” before each existing outcome, preserving the CI-only validation decision. Make dependencies explicit; remove the two stale claims that mobile toolchains were installed locally for 00191 (Problem/Dependencies).

#### B23 — 00197: raw serde conversion cannot preserve the GraphQL wire representation

- **Lenses:** B/C/D. **Location:** 00197, bridge/adoption Features lines 43,48-49 and tasks lines 119,128.
- **Evidence:** Core SchemaApplyReport serializes dry_run (schema_diff/plan.rs:239); GraphQL declares and looks up dryRun (mutations/types.rs:160; base_types.rs:483-490). Compaction's u64 byte counts are GraphQL strings (operations.rs:445-466), whereas raw serde produces numbers.
- **Fails as:** wrong-TDD lock-in / rework thrash. Identity conversion and response-equality acceptance are incompatible.
- **Recommended edit:** require explicit GraphQL name/scalar adaptation preserving the existing schema, while keeping core/REST serialization unchanged. Pin dryRun and the string-encoded byte counts through executed GraphQL responses. Scope “zero adapter edits on new fields” to value serialization; dynamic field declarations still require updates.

#### B24 — 00197: the four-field SQL shape loses DDL messages

- **Lenses:** B/C/D. **Location:** 00197, canonical SQL Feature lines 54-57, exports line 92, Phase 0 task line 120.
- **Evidence:** SqlResult includes Ok(String) (sql_engine/mod.rs:45). GraphQL and FFI expose it as message (base_types.rs:308-312; ffi/records.rs:280-285); CLI prints it (commands/query.rs:35). The proposed columns/rows/column_types/affected shape has no carrier for this existing result.
- **Fails as:** wrong-TDD lock-in. SELECT/DML tests pass while DDL output disappears.
- **Recommended edit:** add message: Option<String> to the canonical shape and pin all three enum variants: query rows, affected count, and DDL message. Declare transport field-name adaptations, including the existing FFI affected_rows name, as part of the compatibility decision.

### Non-blocking

N01-N14 were applied within existing scope. Their original wording is retained below as the correction ledger; none requires a further maintainer answer.

- **N01 — A/C, structural mapping:** Most standard PRDs include the required headings but their Module blocks cover fewer files/capabilities than their trees/tasks. Examples: 00172 defines only the actor module although its tree also owns reload/adapters; 00179 omits the consolidation files; 00181 has no server/FFI module block; 00183 omits a module block for its round-2 sweep/sql_conn decision; 00193 omits its disposition registry module. Complete grouped module definitions and map each Capability and changed file once; do not add implementation scope. The explicit tasks make this repair non-blocking except the missing work in B10.
- **N02 — 00172 structural comments:** Repository Structure still says read_pool call sites migrate and “actor bridge + PgWire call” the reload helper. Correct to the already accepted one actor-bridge trigger and no read_pool ActorHandle migration.
- **N03 — 00177 final conformance paragraph:** It still calls GraphQL the unregistered-type oracle. Restore the accepted CLI/BaseOnly policy oracle and GraphQL warning oracle from its Phase 2 task.
- **N04 — 00180 transaction documentation task:** “Each statement commits independently” holds for autocommit, not all actor-routed transaction calls. PgWire forwards BEGIN/DML/COMMIT/ROLLBACK; service/sql.rs:11-22 preserves transaction state and sql_engine/transaction.rs:19-83 buffers/commits/rolls back. Retain the task's execution-time verification, state exact supported behavior and limitations, and reconcile the outdated architectural description without inventing transaction guarantees.
- **N05 — 00183 old lint-debt prose:** Problem/Target Users still discuss removing helpers lint debt despite the same PRD correctly recording it as already fixed. Remove that residual prose. Preserve the accepted three sweep groups and model-floor exception.
- **N06 — 00184 PgWire rationale:** Its Transport Impact says raw DDL already covers index changes, but ordinary CREATE/DROP INDEX are rejected and CREATE INDEX IF NOT EXISTS is a no-op (sql_engine/mod.rs:323-336,358,407; sql-engine.md:65-67,507-512). Correct the rationale and explicitly keep new constraint operations away from the ignored-index path. The new conformance workflow described in Transport Impact should also be named in the Phase 2 coverage task.
- **N07 — 00185 code constants:** The accepted design and task define strings locally with a test-only core dependency, but the Feature still says reuse core constants and “Do NOT redefine the strings.” Align that prose with the accepted local-definition/parity-test approach, checking both core code modules for the vocabulary. Replace “published crate” with workspace-delivered crate where it could imply registry publication; crates.io publication remains outside the v1 finish line.
- **N08 — 00186 scope ownership:** Its success metric requires jink to run offline codegen, although no task may edit jink. Phrase it as ddb enabling that downstream action and retain the downstream adoption item in the gap ledger. Explicitly include --out and empty-schema behavior in the CLI test acceptance already described by Features.
- **N09 — 00191 remote configuration:** set_remote promises replacement and inspection, but the existing helper only adds a remote (git_ops/remote.rs:6-8). Name replacement/get additions and test replacing an existing remote. This is routine implementation work already implied by the Feature.
- **N10 — 00192 stale alternative:** Phase 1 still says “retire mirror serving or implement its defined guarantee.” Remove the retired alternative; keep redb as already decided. Specify that a mirror failure after a git commit reports partial progress honestly rather than implying the authoritative write never happened.
- **N11 — 00196 transport inventory:** FFI does have execute_sql (ffi/driver.rs:167-169); include it among SQL-bearing consumers. Name service/delete alongside create/update/batch in the adoption checklist if the shared plan covers DELETE as the Feature says; preserve any deliberate delete-specific behavior.
- **N12 — 00198 execution-time matrix:** Phase 0 omits REST although its Feature includes REST. NoSQL CRUD and all six conformance drivers will already exist after 00192/00193; remove “read-only until 00192” as a reason to skip it when this later PRD executes. Name every intended matrix cell in the task, distinguish unsupported inputs on older binaries from corruption, and permit an honestly bounded “not reproduced in tested versions” finding if no fixing commit can be demonstrated.
- **N13 — 00199 minimal template and premises:** The small independent tasks are executable, so retain the minimal form unless it grows. Repeat execution-time no-consumer/stray-marker rechecks in the deletion tasks, with skip-and-report on changed premises. Keep Nice-to-have items genuinely optional; fs2→fs4 must remain conditional on proven equivalence. Ensure relevant docs/walkthrough authoring obligations for its public semantic fixes are explicitly accounted for.
- **N14 — B, validation vocabulary:** Several PRDs say e2e scenarios must be “green locally under Tier 1” (00174/00181), use bare cargo test -p ddb-core (00183), or demand showboat verification per task (00175/00176/00178). Normalize to AGENTS.md's actual Tier 1 commands and CI-owned Tier 2; retain the narrow Claude-routed deletion e2e exception and 00202's specific red-test reproduction exception. Do not delete scenario/test/walkthrough authoring obligations. 00171 must first finish changing the shell-harness mandate.

### Questions

The maintainer delegated routine engineering decisions and resolved the only remaining product tradeoff: this beta has no production clients, so replace incompatible interfaces directly. No questions remain; Q01/Q02 dispositions are recorded below.

- **Q01 — 00186 and later capabilities:** Phase 0 line 125 says every later probe-able capability adds a flag, but later PRDs have no named flag additions. Identify the later public capabilities that need flags and assign the additions to their existing PRDs, or explicitly limit the initial registry to the accepted six-flag seed. This is a producer/consumer completeness question, not a request to invent new features.
- **Q02 — 00183 sql_conn successor:** The exception-exit Feature/task allows delegation to a named successor, but names none. Decide whether an existing later PRD owns the exit, or retain an explicitly documented exception with a tracked follow-up decision. Do not mint a speculative new PRD in this review or silently remove the exception.

## Reshapes

**None required on current evidence.** All PRDs are under 200 lines. 00183's 13-module scope is large but the prior accepted grouping is 10 task lines, under the installed 15-task ceiling; independent code moves need not become separate PRDs merely to satisfy a stylistic size preference.

Measured using installed autopilot 0.4.1 plan-tasks step 4.5: source bytes/4 + full PRD bytes/4 + 1K illustrative task text + 55K overhead. 00183 groups A/B/C estimate **105,597 / 92,245 / 96,785** tokens respectively; 00179's broad sink sweep estimates **94,048**; the principal 00172 verb-migration files estimate **97,344**. These are spot estimates, not generated task plans; exact design/test/caller slices must be re-estimated at planning time. None proves an unavoidable 150K stall. The task budget is separate from the user's 500K session cap; neither was changed.

No cross-PRD cycle or higher-numbered ordinary dependency was found after accounting for completed 00200 and the accepted explicit 00202 run. Later citations into modules split by 00183 are a catchup/re-grounding matter; most affected PRDs already force catchup. 00184-00186 now force catchup to re-ground after the splits; no renumbering was needed. Internal module names and exact signatures delegated to the design phase are not themselves blockers.

## Gaps

- **Execution sequencing remains:** state.json still selects build/00171, task 12 in progress and tasks 13/14 pending. 00202 is now done with the B01 fix committed; finish 00171's own CI and shell-harness migration gates before ordinary backlog draining. This review neither executes that PRD nor changes state.json.
- **Held work still gates the complete product:** 00201's ID-format implementation remains held after the recorded decision; 00168's bundle-import PRD remains held despite substantial implementation already landed. Their lifecycle/review completion is outside this backlog gate. No current backlog PRD has an execution dependency on the parked originals.
- **New app verbs are now assigned:** 00181 owns IndexHealthCommand and its core result; 00191 owns remote/config/sync commands and core outcomes; 00193 explicitly inventories them across all six interfaces.
- **Legacy cleanup is now owned:** 00175 replaces incompatible read/delete responses, 00177 removes public raw-only FFI CRUD exports and migrates callers, and 00178 removes the stringified-fields input path. The beta has no production clients, so no compatibility window or unowned removal milestone remains.
- **sql_conn exit remains deliberately deferred:** 00183 removes the exception only if it fits the approved split scope; otherwise its caller inventory and boundary debt stay documented in traits.rs/design-decisions.md. This audit gap is assigned to the next evolution assessment, without inventing a successor PRD.
- **The unchanged redb decision is not the end of read-store work:** shared read-only handles remain a deferred concurrency follow-up. Keep the per-call rebuild baseline and honest post-commit failure semantics.
- **Downstream adoption and the glue-deletion measurement remain outside ddb implementation:** the v1.0 finish line requires actual jink adoption and measured deletion, not merely client/codegen availability. Retain that external deliverable; do not claim the backlog alone completes v1.0.
- **The 00167/00169 deferred tails remain in dev/local/autopilot/deferred/202607161128-deferred.json.** Reconcile those and held items in a later /assess-evolution pass; that skill was not invoked and no new PRDs were invented here.

Traceability: the July purpose-fit and goal-gap decisions still map to read freshness, mobile replicas, six-interface conformance, bindings CI, and downstream tooling. The July evolution additions map to 00174/00179/00180/00182/00183/00195-00199. Accepted non-goals (LAN discovery, foreign importers, registry publishing, extended PgWire protocol) were not pulled into scope. The contract gaps above concern already promised work.

## End state after this batch

After implementation, the revised batch should leave a closure-based actor, one core error policy, contract-routed CRUD/search, structured inputs, shared SQL write execution, consistent report rendering, six-interface CRUD conformance, bounded server operations, read freshness with opt-in watch, mobile FFI sync with CI-built artifacts, and the client/export/codegen/test tooling downstreams need. It should also remove oversized module barriers and guard batch rewrites against concurrent data loss. The held identity/bundle work, deferred safety tails, the explicitly retained sql_conn exception if not removed within scope, and measured downstream adoption still prevent equating “backlog drained” with the full v1.0 finish line.

## Frontmatter tuning

| PRD | suggestion | why |
|-----|------------|-----|
| 00183 | retain sonnet and its recorded exception | explicitly accepted earlier; per-task classification still raises actual contract/algorithm changes | 183 |  |  |
| 00193/00194/00198/00199 | retain recorded sonnet floors | earlier choices are explicit; risky implementation slices still classify independently |
| other opus PRDs | no blanket change | concurrency, persisted-data transforms, public contracts, or equivalence obligations justify them |

All 28 remaining PRDs record a valid default_model and rationale: **23 opus, 5 sonnet**. No design_gate: user or other explicit mid-loop approval gate is present. All files remain below 200 lines, with at most 10 task lines per PRD; unchanged budget/tier rules were read from the installed plugin.

## Decisions applied

**Maintainer decisions:** handle engineering corrections independently and present only choices requiring product intent. The beta has no production clients; replace incompatible interfaces directly in the next release containing each change. No legacy compatibility window, no waiver of correctness/parity, and no further approval question.

| Finding | Resolution |
|---------|------------|
| B01 | Resolved in the other session by 52759bd/a7e5179; 00202 is done. No completed-PRD edits from this review. |
| B02 | 00195 guards logical units, writes/deletions/expected absence, coherent read snapshots, optional commits, quarantine atomicity, and migration-marker retry safety. |
| B03 | 00173 owns the neutral policy in core, including category/status/redaction/FFI hints; 00177 consumes it without a dependency cycle. |
| B04 | 00174 requires actual abort-capable fetch/push deadlines before and during progress, no detached work, and no repo lease over blocked I/O; REMOTE_TIMEOUT has one core-policy row. |
| B05 | 00176 returns AppOutput<AppSearchResult>, a page of canonical hits plus total/query. |
| B06 | Default 50, max 10,000, offset 0; invalid/negative/overflow/over-max inputs fail explicitly; adapter conversions and changed defaults are documented. |
| B07 | 00175 replaces Boolean/204/string/unit responses directly with warning-bearing results; callers/bindings/fixtures and migration notes change together. |
| B08 | 00178 replaces GraphQL String fields with JSON, migrates variable declarations, and rejects encoded-string inputs; no legacy decoder. |
| B09 | 00177 defines binding-safe native inputs and complete core-result mirrors; 00178 owns shared core fields/unsets normalization with adapter decoding. |
| B10 | 00179 explicitly converts old quoting callers and consolidates validators with preserved caller grammars; structure/tasks cover the sinks. |
| B11 | 00180 enforces 8 MiB before extraction, aligns Json limits, and tests exact/oversized bodies with and without Content-Length. |
| B12 | Equal-length content-independent comparison with subtle; explicit unequal-length rejection; no token-length hiding or timing-test proof claimed. |
| B13 | 00185 uses typed parsing failures for malformed items/rows; no silent dropped items or empty replacement rows. |
| B14 | 00186 embeds the three known probe requirements; cross-repo lifecycle paths are optional evidence. |
| B15 | 00187 resolves actual conflicting identity atomically and tests composite/multiple/racing constraints; Update policy is explicitly excluded. |
| B16 | 00189 separates served read nullability from write inputs and executes/deserializes generated selections including REFERENCES. |
| B17 | 00190 returns READ_FRESHNESS_TIMEOUT through the sole policy instead of stale success; retry behavior and transport assertions are explicit. |
| B18 | 00191 adds core report-producing fetch/push halves and pins direction/count/no-op semantics before FFI adaptation. |
| B19 | 00191 preserves committed local progress after push failure and documents/tests failure stages and retry. |
| B20 | 00192 DELETE returns HTTP 200 with canonical result/warnings; post-git mirror failures report partial progress honestly. |
| B21 | 00193 uses structured FFI creates/updates, read/delete records, and filtered contract search; no removed raw exports. |
| B22 | All five 00194 tasks have Acceptance clauses; platform-toolchain validation belongs to CI. |
| B23 | 00197 preserves existing GraphQL field/scalar encoding through explicit adaptation, with fallible conversion and real response fixtures. |
| B24 | 00197 retains SQL message alongside columns/rows/types/affected and tests Rows/Affected/Ok; FFI affected_rows remains the declared field name. |

**N01-N14:** applied module/capability mapping, stale grounding and policy text, correct SQL/transaction inventory, validation-tier wording, and bounded evidence/deletion-premise checks. Every updated task retains acceptance and per-PRD authoring obligations.

**Q01:** retain the six-flag seed. Later existing producer PRDs own idempotent_create (00187), ephemeral_serve (00188), rust_typedef_codegen (00189), read_freshness (00190), ffi_git_sync (00191), nosql_http_crud (00192), and canonical_sql_results (00197), including build/interface scope and registry/handshake tests.

**Q02:** retain and document the sql_conn exception if removal exceeds the approved split scope; the remaining exit is in the evolution-assessment gap ledger above.

**Apply complete:** 28 backlog PRDs edited in place; no renumbering, new PRDs, lifecycle mutation, product implementation, or autopilot execution by this review. These local PRDs and reports are gitignored. No unanswered decision or pending reshape remains.

## Verification record

- Live law read: review-prd-backlog/SKILL.md; create-prd/SKILL.md and minimal/standard/RPG templates; installed autopilot 0.4.1 plan-tasks steps 4-4.7 and 5.5.
- Goal context read: AGENTS.md, CLAUDE.md, README, project capsule, architectural decisions, app-contract/compatibility guidance, source audits, recent git history, lifecycle inventory, and current autopilot task state.
- Grounding: three read-only subagents returned per-PRD evidence for 00172-00181, 00182-00191, and 00192-00199/00202. Every assignment completed. Main review handled compliance/executability/coherence and all set lenses; code and installed dependency sources substantiate the findings.
- Citation command: python3 /Users/bob/.agents/skills/review-prd-backlog/scripts/check_links.py --root . --json; **0 backlog/wip findings, 0 scan errors**. B14 explains the mandatory cross-repo read hidden by a link-ok waiver.
- Final lens A scan: 28/28 template skeletons/order, valid unique filenames and frontmatter, no duplicate Features or stubs, all 164 tasks with Acceptance, every Capability mapped to a Module, and all PRDs below 200 lines.
- Apply recheck: 0 backlog/wip citation findings, 0 scan errors (85 unrelated historical findings excluded by the skill's scope); final input digest recorded above. Tracked git diff --check passed; the tracked tree is clean at a7e5179 after the other session's B01 commits.
- No cargo builds/tests, historical bisect, live CI queries, external service calls, or mobile builds were run. This is a PRD gate, not implementation or CI certification. Historical CI evidence is retained as historical; current CI status and actual mobile artifacts are not newly verified.
- No lens skipped and no grounding subagent failed. B01 was initially a static counterexample; the other session subsequently committed matching-parent implementation and regression coverage. This review inspected the new code/tests and makes no independent claim about their execution or live CI.

**Final verdict: GO for the revised 28-PRD backlog.** Zero open Blocking findings, zero waivers, and no pending decisions/reshapes. Finish the existing 00171 run and its required CI gate before starting ordinary backlog execution.
