# Design: Offline SDL export and capability/version handshake

## Architecture fit

Two additive surfaces, both reusing existing layers:

- **Offline SDL**: the GraphQL schema is already built by one pure function,
  `ddb_server::schema::build_schema()` (`ddb-server/src/schema/mod.rs:27-32`),
  which takes `ActorHandle` + `ReadPool` + `Vec<TableSchema>` + an optional
  `Arc<SchemaReloader>` and returns `async_graphql::dynamic::Schema`.
  `.sdl()` on that `Schema` walks only the type system (field/type
  definitions) — it never invokes a resolver — so the same construction the
  live server uses at startup (`ddb-server/src/schema.rs` route: `ddb-cli`'s
  `serve` command, `ddb-cli/src/commands/maintenance.rs:273-288`) is reusable
  from `ddb-cli` directly, with no HTTP/pg listener bound. This lands in the
  CLI layer (`ddb-cli/src/commands/schema.rs`), calling into the existing
  `ddb-server` schema module — no new architectural layer.
- **Capability/version handshake**: both `/health` (`ddb-server/src/lib.rs`
  `health_ready`, line 232) and GraphQL are presentation surfaces over one new
  pure data module, `ddb-server/src/capabilities.rs`. This mirrors the
  existing single-source pattern already used for schema-reload version
  (`SchemaReloader::version()`) and error mapping (one table, many
  renderers) — no second source of truth for the flag set.

## Module placement

- **New file** `ddb-server/src/capabilities.rs`: owns the flag set and
  `version()`. Registered as `pub mod capabilities;` in `ddb-server/src/lib.rs:1-17`.
- **Edit** `ddb-server/src/lib.rs`:
  - `health_ready` (line 232-255): add a `"capabilities"` key to the existing
    `serde_json::json!({...})` body. No new `Extension` needed — the function
    takes no capabilities-specific state, it calls `capabilities::capabilities()`
    directly.
  - Add `pub mod capabilities;` to the module list (line 1-17).
- **Edit** `ddb-server/src/schema/queries.rs`: add
  `register_capabilities_field(query)` to the chain in `build_query_fields`
  (after line 43, alongside `register_schema_version_field`). New fn mirrors
  `register_schema_version_field` (lines 364-377) but needs no `ctx.data`
  lookup — it is a pure static field, so it renders identically whether the
  schema was built for a live server or offline (`reloader: None`).
- **New file** `ddb-server/src/schema/capabilities_type.rs` is NOT needed —
  the `Capabilities` GraphQL object type is small enough to inline in
  `queries.rs` next to its field registration (see Interfaces below);
  `type_defs.rs` is for the Doogat/shared domain types, not this.
- **Edit** `ddb-server/src/schema/mod.rs`: add a `pub fn render_sdl(repo_path:
  PathBuf) -> Result<String, String>` helper (async — spawns `ActorHandle` +
  `ReadPool` over the given repo, calls `build_schema(..., reloader: None)`,
  returns `.sdl()`). Exported as `ddb_server::schema::render_sdl`.
- **Edit** `ddb-cli/src/main.rs` (around line 495): add
  `SchemaAction::Sdl { #[arg(long)] out: Option<String> }` to the existing
  `SchemaAction` enum.
- **Edit** `ddb-cli/src/commands/schema.rs`: add the `Sdl` match arm. This is
  the one arm in that file that needs a tokio runtime (every other arm is
  synchronous `DoogatService::open` + call) — construct one locally exactly as
  `maintenance::serve` already does (`ddb-cli/src/commands/maintenance.rs:280-286`:
  `tokio::runtime::Runtime::new()?.block_on(async { ... })`), calling
  `ddb_server::schema::render_sdl(repo_path).await`.

## Interfaces & contracts

```rust
// ddb-server/src/capabilities.rs (new file)

/// Stable-ordered feature-flag set this binary advertises. A flag means
/// "this capability is present in the running binary," not a config toggle —
/// callers do not need credentials or a request to learn it.
pub const FLAGS: &[&str] = &[
    "typed_orderby_base_fields",
    "relation_where_filters",
    "inline_zone_ddl",
    "schema_apply",
    "offline_sdl",
    "capabilities_handshake",
];

/// Returns `FLAGS` as owned `String`s, in the fixed order above.
pub fn capabilities() -> Vec<String> {
    FLAGS.iter().map(|s| s.to_string()).collect()
}

/// The binary's semver version (same value `/health` already reports).
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
```

```rust
// ddb-server/src/lib.rs — health_ready, additive change only

let body = serde_json::json!({
    "status": status,
    "version": env!("CARGO_PKG_VERSION"),
    "uptime_seconds": start.0.elapsed().as_secs(),
    "index_reachable": index_reachable,
    "capabilities": crate::capabilities::capabilities(),
});
```

`capabilities` in the JSON body is a flat array of flag-name strings (not an
object keyed by flag → bool) — `capabilities()` already returns "the flags
present," so a flag's presence in the array *is* the true/false signal; an
object with every flag always `true` would be redundant. (The PRD's Outputs
line `capabilities: {...}` names the shape informally; this array is the
simplest encoding of the same contract and the GraphQL field below returns
the identical list under `flags`, so the two surfaces agree by construction.)

```rust
// ddb-server/src/schema/queries.rs — new GraphQL object + field
//
// Matches the existing nested-object precedent used throughout this file
// (e.g. discovery_queries.rs:43,84,140,183,294,382): the parent field
// resolves to an owned `GqlValue::Object` carrying real data, and each
// child field reads it back out of `ctx.parent_value` — NOT an independent
// static lookup, and NOT `GqlValue::Null` (illegal for a `named_nn` field;
// async-graphql raises a field error on a non-null field resolving to null —
// dispatch-2 review finding, cardinal/blocker-grade, fixed here).
fn register_capabilities_field(query: Object) -> Object {
    let capabilities_object = Object::new("Capabilities")
        .field(Field::new("version", TypeRef::named_nn(TypeRef::STRING), |ctx| {
            FieldFuture::new(async move {
                let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                let GqlValue::Object(obj) = obj else { return Ok(None) };
                Ok(obj.get("version").cloned().map(FieldValue::owned_any))
            })
        }))
        .field(Field::new(
            "flags",
            TypeRef::named_nn_list_nn(TypeRef::STRING),
            |ctx| {
                FieldFuture::new(async move {
                    let obj = ctx.parent_value.try_downcast_ref::<GqlValue>()?;
                    let GqlValue::Object(obj) = obj else { return Ok(None) };
                    let Some(GqlValue::List(flags)) = obj.get("flags").cloned() else {
                        return Ok(Some(FieldValue::list(Vec::<FieldValue>::new())));
                    };
                    Ok(Some(FieldValue::list(flags.into_iter().map(FieldValue::owned_any))))
                })
            },
        ));
    query.field(Field::new(
        "capabilities",
        TypeRef::named_nn("Capabilities"),
        |_ctx| {
            FieldFuture::new(async move {
                let mut obj = IndexMap::new();
                obj.insert(Name::new("version"), GqlValue::from(crate::capabilities::version()));
                obj.insert(
                    Name::new("flags"),
                    GqlValue::List(
                        crate::capabilities::capabilities()
                            .into_iter()
                            .map(GqlValue::from)
                            .collect(),
                    ),
                );
                Ok(Some(FieldValue::owned_any(GqlValue::Object(obj))))
            })
        },
    ))
    // NOTE for the implementing task: `Capabilities` must also be pushed
    // into `QueryOutput.dynamic_types` so `build_schema` registers the
    // object type (same mechanism `per_type_queries::register_dynamic_type_fields`
    // uses for typed query results) — extend `build_query_fields`'s
    // `QueryOutput` construction to include it, do not register it as a
    // bare field with no backing type registration.
}
```

`QueryOutput` (`queries.rs:22-30`) gains no new field — `capabilities_object`
is pushed into the existing `dynamic_types: Vec<Object>` returned from
`build_query_fields`, the same vector `per_type_queries` already populates
for typed result objects (line 46-55), so `build_schema`'s existing
`for t in q.dynamic_types { builder = builder.register(t); }`-style loop (if
present) or equivalent `.register()` call picks it up with no new wiring in
`build_schema` itself.

```rust
// ddb-cli/src/main.rs — SchemaAction, additive variant

#[derive(Subcommand)]
pub(crate) enum SchemaAction {
    Apply { file: String, #[arg(long)] dry_run: bool, #[arg(long)] allow_destructive: bool },
    Diff { file: String },
    /// Print the GraphQL SDL for the repo's installed typedefs, offline.
    Sdl {
        /// Write SDL to this file instead of stdout.
        #[arg(long)]
        out: Option<String>,
    },
}
```

```rust
// ddb-server/src/schema/mod.rs — offline render helper

/// Builds the same dynamic schema the live server builds, over `repo_path`,
/// with no reloader and no network listener bound, and returns its SDL.
pub async fn render_sdl(repo_path: std::path::PathBuf) -> Result<String, String> {
    use crate::actor::ActorHandle;
    use crate::events::EventBus;
    use crate::read_pool::ReadPool;
    use ddb_core::service::DoogatService;

    let type_schemas = DoogatService::open(&repo_path)
        .map_err(|e| e.to_string())?
        .list_type_schemas()
        .map_err(|e| e.to_string())?;

    let event_bus = EventBus::new();
    let actor = ActorHandle::spawn(
        repo_path.clone(),
        event_bus,
        ddb_core::git_ops::DEFAULT_REMOTE_TIMEOUT,
    )
    .map_err(|e| e.to_string())?;
    let read_pool = ReadPool::new(repo_path, 1).map_err(|e| e.to_string())?;

    let schema = build_schema(actor, read_pool, type_schemas, None)?;
    Ok(schema.sdl())
}
```

```rust
// ddb-cli/src/commands/schema.rs — Sdl arm

SchemaAction::Sdl { out } => {
    let repo_path = std::fs::canonicalize(repo)?;
    let rt = tokio::runtime::Runtime::new().map_err(ddb_core::error::DoogatError::Io)?;
    let sdl = rt
        .block_on(ddb_server::schema::render_sdl(repo_path))
        .map_err(ddb_core::error::DoogatError::Other)?; // or the crate's existing string->error conversion
    match out {
        Some(path) => std::fs::write(path, sdl)?,
        None => outln!("{sdl}")?,
    }
    Ok(())
}
```

(`ddb_core::error::DoogatError::Other` is illustrative — the implementing
task should use whichever existing `DoogatError` variant the codebase already
uses for a plain `String` error at this boundary; `build_schema` itself
already returns `Result<Schema, String>`, so one such variant exists
somewhere in the CLI's error-conversion call sites.)

## Data flow

**Offline SDL**: `ddb schema sdl` (CLI) → `ddb_server::schema::render_sdl`
(spawns an in-process `ActorHandle` + `ReadPool` over the repo path, no bind)
→ `DoogatService::list_type_schemas()` (sync, reused as-is) →
`build_schema(actor, read_pool, type_schemas, reloader: None)` (reused as-is)
→ `Schema::sdl()` → stdout or `--out` file.

**Live GraphQL SDL** (for the conformance test, unchanged): `ddb serve` →
`ddb_server::run` → `build_initial_schema` → same `build_schema` → introspection
query → SDL string. Both paths converge on the one `build_schema` call, so
drift is structurally prevented, not just tested.

**Capability handshake**: `capabilities::capabilities()` / `version()` (pure,
no I/O) is read by two call sites — `health_ready` (HTTP) and
`register_capabilities_field`'s resolvers (GraphQL) — each wrapping the same
two function calls in its own response shape. No new state, no actor
round-trip: this is why the GraphQL field needs no `ctx.data` lookup and
therefore renders correctly even through the headless offline-SDL path (it
would, if ever queried there, which it structurally cannot be since
`.sdl()` never executes resolvers).

## Reuse inventory

- `ddb_server::schema::build_schema()` (`ddb-server/src/schema/mod.rs:27`) —
  the one schema constructor; offline SDL calls it unchanged with
  `reloader: None`.
- `async_graphql::dynamic::Schema::sdl()` — stdlib-of-the-dependency; no
  custom SDL printer needed.
- `ddb_core::service::DoogatService::list_type_schemas()`
  (`ddb-core/src/service/utility.rs:49`) — already the canonical typedef
  source `ddb-server`'s own startup path uses (`ddb-server/src/schema.rs:61`
  via `actor.get_type_schemas()`, which itself proxies to the service).
  Offline SDL calls the sync service method directly rather than round-tripping
  through the actor, since no actor command channel is needed just to list
  typedefs.
- `tokio::runtime::Runtime::new().block_on(...)` pattern — already used
  verbatim by `maintenance::serve` (`ddb-cli/src/commands/maintenance.rs:280-286`)
  for the one other place `ddb-cli` needs to drive async `ddb-server` code;
  the `Sdl` arm copies this pattern rather than inventing a second one.
- `register_schema_version_field` (`ddb-server/src/schema/queries.rs:364-377`)
  — the closest existing precedent for a static, no-argument GraphQL field;
  `register_capabilities_field` follows its shape.
- `env!("CARGO_PKG_VERSION")` — already used in `health_ready`
  (`ddb-server/src/lib.rs:249`); `capabilities::version()` wraps the same
  macro call so there is exactly one place version is read from, matching the
  PRD's "do not rename/duplicate the `version` key" risk mitigation.
- Greps tried (nothing else found): `rg "capabilit"` (only this PRD's own
  file and the two integration points named above), `rg "feature.?flag"`,
  `rg "fn sdl\b"`, `rg "render_sdl|export_sdl|schema_sdl"` — all return
  nothing pre-existing beyond what is cited above, confirming no prior
  partial implementation to reconcile.

## Alternatives considered

1. **Smallest-diff: shell out to a running server's introspection endpoint
   and cache the result.** Rejected — this is exactly the live-server
   dependency the PRD exists to remove; it would still require a token and a
   bound port at export time, defeating "no `ddb serve`, no token, no bound
   listener."
2. **Hand-write/maintain a static SDL string or a second, simplified schema
   builder for offline use.** Rejected — guarantees drift between offline and
   served SDL (the PRD's own named risk), and duplicates logic `build_schema`
   already owns correctly.
3. **Chosen: reuse `build_schema` directly, with `reloader: None` and a
   throwaway in-process `ActorHandle`/`ReadPool` that never binds a listener.**
   This is also the smallest actual diff once `build_schema`'s existing
   signature is taken as fixed (option 1's "shell out" is smaller in line
   count but solves the wrong problem; option 2 is larger and strictly worse).
   The one piece of friction — `build_schema` wants resolver handles at all —
   is accepted as-is per the PRD's own risk mitigation ("choose the smaller
   change": decoupling type-system construction from resolver wiring would be
   a larger, speculative refactor for a single offline caller).

## Risks & edge cases

- **`ActorHandle::spawn` does real work** (opens the repo, spawns a
  background rebuild-if-stale thread) even though offline SDL never issues a
  write or a read through it. This is bounded and matches an existing
  acceptable cost (the same spawn the live server pays at every startup); no
  mitigation needed beyond what already exists, but a reviewer should not
  mistake this for "building a server" — no socket is opened anywhere in this
  path.
- **Empty-typedef repos**: `list_type_schemas()` returning `[]` must still
  produce valid base SDL (Query/Mutation root types with no dynamic
  fields) — this is already proven by `build_schema`'s own test suite
  (`build_schema_includes_hyphenated_type` and friends construct from
  arbitrary `TableSchema` sets); the empty case is not materially different
  and needs its own explicit conformance assertion per the PRD's Critical
  Scenarios.
- **Likely next changes** (per the PRD's own "Later flag ownership" list —
  00187 idempotent_create, 00188 ephemeral_serve, 00189 rust_typedef_codegen,
  00190 read_freshness, 00191 ffi_git_sync, 00192 nosql_http_crud, 00197
  canonical_sql_results): each will append one flag to `capabilities::FLAGS`
  and nothing else. This design keeps `FLAGS` a flat `&[&str]` precisely so
  those PRDs' diffs are one-line insertions, not schema changes — adding a
  flag never touches `health_ready`, the GraphQL resolver, or the SDL export
  path.
- **Feature-gated builds**: the PRD requires flags to reflect "present in
  this binary," so a future compile-time-feature-gated capability must guard
  its entry in `FLAGS` with `#[cfg(feature = "...")]` rather than appending it
  unconditionally — out of scope for 00186 (no such feature exists yet) but
  worth a one-line doc comment on `FLAGS` so the pattern is discoverable when
  needed.
- **`--out` plus stdout double-write**: the PRD's acceptance criterion
  requires `--out` to write identical content "without duplicating it on
  stdout" — the `Sdl` arm above branches `Some(path) => write-only` /
  `None => stdout-only`, never both; a reviewer should check no `eprintln!`
  echo was added alongside the file write (other commands in this CLI do
  sometimes echo both to stderr and a file, which would violate this
  acceptance criterion here).

## Test strategy outline

- **Unit** (`ddb-server/src/capabilities.rs`): pins the exact initial `FLAGS`
  set and order (Phase 0 task's own acceptance criterion).
- **Unit** (`ddb-server/src/lib.rs` or a colocated test module): `health_ready`
  response includes `capabilities` as an array equal to
  `capabilities::capabilities()`, and still includes the unchanged `version`
  key.
- **Integration/conformance** (`ddb-core`/`ddb-server` test or `tests/e2e/`):
  build a fixture `Vec<TableSchema>`, call `render_sdl`-equivalent offline
  construction and the live `build_initial_schema` path with the *same*
  fixture, assert `.sdl()` output is identical (normalized whitespace if
  needed) — the PRD's named conformance pin.
- **e2e** (`tests/e2e/`, Rust harness per CLAUDE.md): a `smoke_` scenario for
  `ddb schema sdl` against a repo with typedefs (non-empty SDL, contains the
  typed query/object names) and against a fresh repo with none (valid base
  SDL, no panic); an `integration_` scenario (or the same smoke file) hitting
  `/health` and the GraphQL `capabilities` query and asserting both report
  the same `version` + flag set.
- **Docs**: `docs/src/technical/server.md` (health/capabilities section),
  `docs/src/guide/building-apps.md` (offline codegen replacing the live-server
  prerequisite jink's `codegen.ts` comment describes).
- **Walkthrough**: new CLI command (`ddb schema sdl`) per repo convention —
  `dev/local/walkthroughs/` via showboat, verified with
  `dev/bin/safe-showboat-verify`, never from the project root.

## Review log

- non-blocker: `capabilities` field's resolver pattern is reloader-independent, but the cited precedent `register_schema_version_field` (`queries.rs:364-377`) depends on `ctx.data::<Arc<SchemaReloader>>`, which is absent when `build_schema` is called with `reloader: None` (as `render_sdl` does). Not a crash risk for offline SDL (`.sdl()` never executes resolvers), but worth noting that `reloader: None` schemas are SDL-only-safe, not query-safe, so a future caller must not reuse `render_sdl`'s construction for anything beyond `.sdl()`.
- question (superseded by dispatch-2 blocker fix below): the original draft's `register_capabilities_field` returned `FieldValue::value(GqlValue::Null)` as the parent marker; dispatch 2 confirmed this was actually illegal for a `named_nn` field and not just unverified. Fixed — see the dispatch-2 finding.
- non-blocker: `ActorHandle::spawn`'s cold-start `rebuild_if_stale` race (tracked in memory `project_index_rebuild_concurrency_race`, fix merged into backlog 00169 but not yet shipped) is paid unconditionally by offline SDL export. Running `ddb schema sdl` concurrently with another process against the same repo hits this same unresolved availability gap — not a new risk this design introduces, but worth a cross-reference rather than treating the spawn cost as fully benign.
- question: `render_sdl` always pays a full `ActorHandle::spawn` (git-open + background rebuild thread) even though `.sdl()` never executes resolvers and so never reads the actor's value — `build_schema`'s `.data(actor)` only needs a type-satisfying container. Whether a lighter non-spawning `ActorHandle` stub is feasible without touching its public API is unexplored; if infeasible, document explicitly why a real spawn is necessary rather than merely assumed necessary.
- dispatch 1 (claude): cardinal-sin 0, blocker 0, non-blocker 2, question 2
- dispatch 2 codex outage (sandbox denied all tool execution, output unparseable as substantive findings) — Claude fallback used.
- blocker: the draft's `capabilities` parent field resolved `TypeRef::named_nn("Capabilities")` to `GqlValue::Null`, illegal for a non-null GraphQL field (async-graphql raises a field error on this shape; no existing nested-object precedent in this codebase resolves a parent to Null). Fixed: parent now resolves to an owned `GqlValue::Object` carrying real data (`version`, `flags`), matching the `discovery_queries.rs` precedent; child fields read it back via `ctx.parent_value.try_downcast_ref::<GqlValue>()`.
- question: the PRD's own Output line (`capabilities: {...}`, and the health_ready bullet's literal `{ "<flag>": true, ... }` shape) describes an object-of-booleans, while this design ships `capabilities` as a flat array of present-flag names on both `/health` and GraphQL. The design's rationale (presence-in-array already encodes true/false) is reasonable but is an explicit deviation from the PRD's literal acceptance text; no downstream (jink) consumer code exists yet to confirm which shape it expects. Recorded, not fixed — the array shape is this design's explicit decision (see `## Interfaces & contracts`); a planner should treat it as settled unless PRD ownership says otherwise.
- dispatch 2 (claude-fallback): cardinal-sin 0, blocker 1, non-blocker 0, question 1
- dispatch 3 codex outage (sandbox denied all tool execution again) — Claude fallback verified the blocker-1 fix (parent/child `GqlValue::Object` pattern matches `discovery_queries.rs` precedent exactly, types consistent) and found nothing else open.
- dispatch 3 (claude-fallback): cardinal-sin 0, blocker 0, non-blocker 0, question 0
result: ok
