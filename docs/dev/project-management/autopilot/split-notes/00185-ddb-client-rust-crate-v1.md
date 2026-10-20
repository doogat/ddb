# Split note: 00185-ddb-client-rust-crate-v1.md
planned=5 prd_tasks=4 expansion=1.25 reasons=module_drift

## . (UNLISTED)

- 1 Scaffold ddb-client crate + typed error-code enum + parity test: Cargo.toml
- 5 ddb-client crate integration tests + docs + CHANGELOG: CHANGELOG.md

## ddb-client/tests (UNLISTED)

- 1 Scaffold ddb-client crate + typed error-code enum + parity test: ddb-client/tests/codes_parity.rs
- 5 ddb-client crate integration tests + docs + CHANGELOG: ddb-client/tests/integration.rs

## docs/src/guide (UNLISTED)

- 5 ddb-client crate integration tests + docs + CHANGELOG: docs/src/guide/building-apps.md

## docs/src/technical (UNLISTED)

- 5 ddb-client crate integration tests + docs + CHANGELOG: docs/src/technical/client.md

## ddb-client (listed)

- 1 Scaffold ddb-client crate + typed error-code enum + parity test: ddb-client/Cargo.toml

## ddb-client/src (listed)

- 1 Scaffold ddb-client crate + typed error-code enum + parity test: ddb-client/src/lib.rs, ddb-client/src/error.rs
- 2 Implement ddb-client response parsers (envelope, connection items, nested, SQL result): ddb-client/src/parse.rs, ddb-client/src/lib.rs
- 3 Implement DdbClient authenticated transport (typed_query, execute_sql, query_sql): ddb-client/src/transport.rs, ddb-client/src/lib.rs
- 4 Implement wait_ready() backoff + finalize DdbClient facade re-exports: ddb-client/src/readiness.rs, ddb-client/src/lib.rs
