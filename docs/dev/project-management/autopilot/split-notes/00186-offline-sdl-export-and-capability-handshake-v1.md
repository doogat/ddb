# Split note: 00186-offline-sdl-export-and-capability-handshake-v1.md
planned=5 prd_tasks=5 expansion=1.00 reasons=module_drift

## . (UNLISTED)

- 5 Add e2e scenarios, docs, CHANGELOG, and walkthrough for offline SDL + capabilities: CHANGELOG.md

## docs/src/guide (UNLISTED)

- 5 Add e2e scenarios, docs, CHANGELOG, and walkthrough for offline SDL + capabilities: docs/src/guide/building-apps.md

## docs/src/technical (UNLISTED)

- 5 Add e2e scenarios, docs, CHANGELOG, and walkthrough for offline SDL + capabilities: docs/src/technical/server.md

## tests/e2e (UNLISTED)

- 5 Add e2e scenarios, docs, CHANGELOG, and walkthrough for offline SDL + capabilities: tests/e2e/main.rs, tests/e2e/smoke_schema_sdl.rs

## ddb-cli/src (listed)

- 2 Add offline render_sdl helper + `ddb schema sdl` CLI command: ddb-cli/src/main.rs

## ddb-cli/src/commands (listed)

- 2 Add offline render_sdl helper + `ddb schema sdl` CLI command: ddb-cli/src/commands/schema.rs

## ddb-server/src (listed)

- 1 Add capabilities module (version + feature-flag set): ddb-server/src/capabilities.rs, ddb-server/src/lib.rs
- 3 Add capabilities to /health and a GraphQL capabilities field: ddb-server/src/lib.rs

## ddb-server/src/schema (listed)

- 2 Add offline render_sdl helper + `ddb schema sdl` CLI command: ddb-server/src/schema/mod.rs
- 3 Add capabilities to /health and a GraphQL capabilities field: ddb-server/src/schema/queries.rs
- 4 Conformance test: offline SDL equals served SDL: ddb-server/src/schema/mod.rs
