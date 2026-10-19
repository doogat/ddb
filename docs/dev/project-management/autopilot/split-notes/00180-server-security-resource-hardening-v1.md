# Split note: 00180-server-security-resource-hardening-v1.md
planned=9 prd_tasks=9 expansion=1.00 reasons=module_drift

## . (UNLISTED)

- 9 Docs and CHANGELOG for server security/resource hardening: CHANGELOG.md

## ddb-core/src (UNLISTED)

- 4 NoSQL scan/backlinks pagination cap: ddb-core/src/nosql.rs

## ddb-server (UNLISTED)

- 2 Router-wide request body limit (REST, GraphQL, NoSQL): ddb-server/Cargo.toml
- 5 Constant-time bearer token compare: ddb-server/Cargo.toml

## ddb-server/src (UNLISTED)

- 1 GraphQL depth + complexity limits and introspection gate: ddb-server/src/reload.rs, ddb-server/src/lib.rs, ddb-server/src/config.rs
- 2 Router-wide request body limit (REST, GraphQL, NoSQL): ddb-server/src/lib.rs, ddb-server/src/config.rs
- 3 PgWire SELECT row cap: ddb-server/src/pgwire.rs
- 4 NoSQL scan/backlinks pagination cap: ddb-server/src/nosql_api.rs
- 5 Constant-time bearer token compare: ddb-server/src/auth.rs
- 6 Non-loopback bind startup warning: ddb-server/src/config.rs, ddb-server/src/lib.rs
- 7 PgWire error redaction parity: ddb-server/src/pgwire.rs

## docs/src/technical (UNLISTED)

- 9 Docs and CHANGELOG for server security/resource hardening: docs/src/technical/server.md

## tests/e2e (UNLISTED)

- 8 e2e scenarios for server security/resource hardening: tests/e2e/integration_server_security_hardening.rs, tests/e2e/main.rs, tests/e2e/integration_nosql_cli.rs

## ddb-core/src/service (listed)

- 4 NoSQL scan/backlinks pagination cap: ddb-core/src/service/utility.rs

## ddb-server/src/schema (listed)

- 1 GraphQL depth + complexity limits and introspection gate: ddb-server/src/schema/mod.rs
