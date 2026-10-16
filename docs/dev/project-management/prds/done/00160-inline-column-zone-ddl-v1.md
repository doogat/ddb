# Inline column ZONE at CREATE TABLE

## Overview

### Problem Statement
A column's storage zone (frontmatter / body / reference) is derived implicitly at CREATE time — `ddb-core/src/sql_engine/ddl.rs:267-274` (references => Reference; `is_numeric_type || is_short_string_type` => Frontmatter; else Body), with the default fallback in `ddb-core/src/types/schema.rs:57-66` (`effective_zone`). There is no way to declare a column's zone inline: the column-option extractors (`ddb-core/src/sql_engine/helpers.rs:281-384`) include no zone extractor. The only explicit control is post-hoc `ALTER TABLE … SET ZONE … FOR …` (regex `helpers.rs:12-15`, handler `ddl.rs:754-781`).

### Target Users
Any downstream declaring `TEXT`/`REFERENCES` columns that belong in frontmatter — i.e. most schemas with descriptive text or denormalized references.

### Success Metrics
- A column can declare its zone at create time (e.g. `descr TEXT ZONE frontmatter`), persisted identically to a post-hoc `SET ZONE`.
- Downstreams drop their post-hoc `SET ZONE` migration blocks.
- Default behavior is unchanged when no zone is declared, and the syntax round-trips through introspection / `ddb fix`.

Motivating downstream evidence (jink): every `TEXT`/`REFERENCES` column that should live in frontmatter needs a follow-up `SET ZONE` migration; jink carries 7 such statements purely to correct defaults (`src/schema.rs:96-102`), and omitting them silently writes the literal string "NULL".

## Functional Decomposition

### Capability: Create-time zone declaration
#### Feature: Column ZONE attribute
Parse and apply an inline zone declaration on a column in `CREATE TABLE`.
#### Feature: Default preservation
When no zone is declared, the existing implicit derivation is used unchanged.

## Structural Decomposition

### Repository Structure
SQL DDL engine (parse + apply); no GraphQL change.

### Module: ddb-core/src/sql_engine/helpers.rs
Add `extract_zone(options)`. sqlparser will not model `ZONE` natively, so this likely needs a pre-parse regex (mirroring `re_set_zone`) or a `ColumnOption::DialectSpecific`/comment token.

### Module: ddb-core/src/sql_engine/ddl.rs
Consume the extracted zone at the derivation site (267-274), ahead of the implicit fallback; persist via the same path as `SET ZONE`.

## Dependency Graph

### Foundation Layer (Phase 0)
Syntax decision + extractor.

### Core Layer (Phase 1)
Wire into CREATE zone derivation + persistence.

### Integration Layer (Phase 2)
Introspection round-trip + tests.

## Implementation Phases

### Phase 0: Foundation
Decide the inline syntax and add `extract_zone` in `helpers.rs`.

### Phase 1: Core
Consume the extracted zone at `ddl.rs:267` before the implicit fallback; persist identically to `SET ZONE`. Optionally expose a separate toggle to reconsider the scalar-`TEXT` default (=> frontmatter).

### Phase 2: Integration
Ensure the declaration round-trips through introspection and `ddb fix`; add tests.

## Test Strategy

### Critical Scenarios
- Create columns with explicit zone per kind (TEXT / REFERENCES / numeric); verify materialization.
- Omit zone: default derivation unchanged.
- Rematerialization correctness after a create-time zone.
- Round-trip: introspect / `ddb fix` reproduce the declared zone.

## Risks
- DDL syntax must round-trip through introspection and `ddb fix`.
- sqlparser limitation forces a regex pre-parse, which must not misfire on similar tokens.

## Transport Impact
SQL DDL via `executeSql` and the CLI. GraphQL unaffected; the resulting zone shows through existing introspection.
