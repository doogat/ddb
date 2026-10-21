# Assumptions ledger

## Task 3 - Add capabilities to /health and a GraphQL capabilities field

- Tess: The e2e crate (`tests/Cargo.toml`) does not depend on ddb-server, so the e2e tests pin the six flag strings themselves (copied from `FLAGS`) and check the version against `env!("CARGO_PKG_VERSION")` (both crates are 0.2.7 today). They also compare `/health` with GraphQL on the same server, instead of calling `ddb_server::capabilities` directly.
- Tess: `/health/ready` shares the `health_ready` handler, so it is expected to carry `capabilities` too.
- Tess: The "existing integration test harness" for `/health` is the e2e `ServerGuard`, used by `serve.rs`.
- Devon: The intended field types are non-null (`String!`, `[String!]!`); the tests themselves never state so directly.
- Tess (strengthen): The resolver calls the qualified paths `capabilities::version()` and `capabilities::capabilities()`; a bare `use` plus unqualified call would fail the source-scan assertions.
- Ivan: `capabilities()` returns items that can be turned into strings via `.to_string()`; `version()` returns `&str`.
