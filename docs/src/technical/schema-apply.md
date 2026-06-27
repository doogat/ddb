# Declarative Schema Apply

Declarative schema apply lets a downstream declare its typedefs once, in a single
schema document, and call one apply. Doogat DB diffs the desired schema against the
live schema and runs the minimal safe migration. This replaces hand-written, ordered
`CREATE`/`ALTER` sequences and the per-app idempotency and detection logic that go
with them.

The imperative DDL path (`CREATE TABLE`, `ALTER TABLE`, `DROP TABLE` via `ddb query`,
GraphQL `executeSql`, or PgWire) still works unchanged. Declarative apply sits on top
of it: the differ renders its plan into the same DDL grammar the
[SQL Engine](./sql-engine.md) already executes.

## The desired-schema document

The desired schema is a YAML document listing one or more typedefs. Each type body
uses the same vocabulary a typedef or `CREATE TABLE` already uses (`columns`, `zone`,
`search_key`, `singleton`, `unique_together`, `title_template`, `folder`). There is no
new DSL to learn.

```yaml
types:
  - name: contact
    columns:
      - name: email
        data_type: VARCHAR(255)
        zone: frontmatter
        required: true
      - name: notes
        data_type: TEXT
        zone: body
    search_key: email          # optional; default is the title
    singleton: false           # optional; default false
    unique_together: [[email]] # optional
```

`SchemaDoc::from_yaml` (in `ddb-core/src/schema_diff/desired.rs`) parses each entry by
reusing `schema_from_parsed`, the same assembly the typedef parser uses, so the desired
and stored vocabularies cannot drift.

## Diff and plan

`schema_diff::diff` is a pure function: it takes the desired types plus the live types
and returns an ordered `SchemaPlan`. The differ matches columns by name and emits one op
per change. Plan op kinds:

| Op kind | Meaning | Rendered DDL |
|---|---|---|
| `create_type` | type does not exist yet | `CREATE TABLE ...` (inline `ZONE`, `unique_together`, singleton) |
| `add_column` | column in desired, not live | `ALTER TABLE ... ADD COLUMN ...` |
| `alter_column_type` | `data_type` differs | `ALTER TABLE ... ALTER COLUMN ... TYPE ...` |
| `set_zone` | effective storage zone differs | `ALTER TABLE ... SET ZONE ...` |
| `set_search_key` | `search_key` differs | `ALTER TABLE ... SET SEARCH KEY ...` (or `DROP SEARCH KEY`) |
| `set_singleton` | `singleton` flag differs | `ALTER TABLE ... SET SINGLETON` (or `DROP SINGLETON`) |
| `rename_column` | explicit `rename_from` directive | `ALTER TABLE ... RENAME COLUMN ...` (destructive) |
| `drop_column` | column in live, not desired | `ALTER TABLE ... DROP COLUMN ...` (destructive) |

Ops apply in a safe order: per-type create, add, alter, zone, search-key, and singleton
changes first, then every rename, then every drop last.

### Dry-run

`--dry-run` (and the `diff` alias) compute the plan and print it without mutating
anything. Each line is `<kind> <table> destructive=<bool> -- <detail> -- <sql>`:

```text
create_type contact destructive=false -- create type contact -- CREATE TABLE contact (email VARCHAR(255) NOT NULL ZONE frontmatter, notes TEXT ZONE body)
```

`ddb schema diff <file>` is byte-identical to `ddb schema apply <file> --dry-run`; both
render through the one shared plan path.

## Apply

A real apply (no `--dry-run`) runs the plan. Apply is **atomic**: every op's typedef
write is buffered in one transaction and flushed as a single git commit on success. If
any op fails, the whole transaction rolls back, so a partially-applied plan never reaches
git and `HEAD` is unchanged. Reads inside the transaction see prior buffered writes
(read-your-writes), so adding several columns to the same existing type in one apply
preserves every column.

Apply is **idempotent**. A second apply re-diffs against the current live schema; once
converged the plan is empty and apply reports `no changes` (the report's `applied` flag
is `false`). `create_type` is only emitted when the type is absent, so re-apply never
re-creates an existing type.

### The destructive gate

`drop_column` and `rename_column` may lose data, so they are gated. Without
`--allow-destructive` an apply whose plan contains a destructive op is refused before any
mutation, with error code `SCHEMA_DESTRUCTIVE_BLOCKED`:

```text
error: schema plan contains destructive operations (drop/rename); re-run with allow_destructive
```

Pass `--allow-destructive` to permit drops and renames. A rename uses the explicit
`rename_from: <old>` directive on the column (renames are never inferred from a
drop-plus-add pair, which is ambiguous) and preserves the column's row data under the new
name.

### Unsupported changes are surfaced, never dropped

Some desired-vs-live differences have no targeted ALTER DDL. The differ still detects them
and returns each as a warning with code `SCHEMA_UNSUPPORTED_CHANGE`; they are never
silently ignored. Unsupported changes include per-column `required`, `default_value`,
`references`, `allowed_values`, `on_delete`, and `search_boost`, and per-type
`unique_together`, `title_template`, `folder`, `crdt_strategy`, `template_sections`, and
`stale_after_days`.

### What "index" means here

Doogat DB has no general secondary indexes to declare. Full-text search is automatic
(FTS5 over every doogat). The one declarable index-like constraint is `unique_together`,
which creates SQLite unique indexes at materialization. `unique_together` is only settable
at `CREATE TABLE`, so a change to it on an existing type is reported as an unsupported
change rather than applied.

## Error and warning codes

| Code | Kind | Meaning |
|---|---|---|
| `SCHEMA_DESTRUCTIVE_BLOCKED` | error | plan contains a drop or rename and `allow_destructive` was not set; nothing mutated |
| `SCHEMA_APPLY_PARTIAL` | error | an op failed mid-plan; the whole plan rolled back. On CLI and FFI the error context lists the ops that ran before the failure (`applied_ops`) and the failing op (`failed_op`); on GraphQL and REST the detailed message and context are redacted (see note below) |
| `SCHEMA_UNSUPPORTED_CHANGE` | warning | a desired-vs-live difference with no ALTER DDL path; surfaced on every transport |

On GraphQL and REST, `SCHEMA_APPLY_PARTIAL` is in the Internal error category, so the detailed
message and the `applied_ops` / `failed_op` context are redacted (REST returns it as HTTP 500);
the full context stays available on CLI and FFI. Because a partial apply rolls the whole plan
back, re-applying the same desired-schema document is a safe, idempotent no-op (an already-converged
schema produces an empty plan).

## Interfaces

One service verb, `DoogatService::apply_schema`, backs four Guaranteed adapters. PgWire
and NoSQL HTTP are intentionally absent.

| Interface | Disposition | Surface |
|---|---|---|
| CLI | Guaranteed | `ddb schema apply <file> [--dry-run] [--allow-destructive]`; `ddb schema diff <file>` |
| GraphQL | Guaranteed | `applySchema(schema: String!, dryRun: Boolean, allowDestructive: Boolean): SchemaApplyReport!` |
| REST | Guaranteed | `POST /rest/schema/apply` body `{schema, dryRun, allowDestructive}` returns `{data: SchemaApplyReport, warnings: []}` |
| FFI | Guaranteed | `DoogatDriver::apply_schema(schema_doc, dry_run, allow_destructive) -> SchemaApplyReportRecord` |
| PgWire | Intentionally absent | PgWire is a SQL wire protocol; its equivalent is the imperative DDL it already executes. Declarative apply is a higher-level op with no SQL surface. |
| NoSQL HTTP | Intentionally absent | Read-only key/value document surface; schema management is out of its workflow. |

The FFI adapter returns a typed `SchemaApplyReportRecord` (UniFFI record) rather than a
JSON string, because `ddb-core` deliberately keeps a JSON serializer out of its required
dependency surface. The record carries the same fields as the JSON report the other
transports return.

## Module map

| Concern | Location |
|---|---|
| Desired-schema parse | `ddb-core/src/schema_diff/desired.rs` (`SchemaDoc::from_yaml`) |
| Plan + rendering + report DTO | `ddb-core/src/schema_diff/plan.rs` (`PlanOp`, `SchemaPlan`, `SchemaApplyReport`) |
| Pure differ | `ddb-core/src/schema_diff/differ.rs`, `mod.rs` (`diff`, `diff_type`) |
| Service verb | `ddb-core/src/service/schema_apply.rs` (`apply_schema`, `describe_type`) |
| App-contract command | `ddb-core/src/app_contract/commands.rs` (`ApplySchemaCommand`) |

The differ is pure (no I/O), which keeps the diff logic unit-testable in isolation; the
service verb gathers the live schemas, runs the differ, and applies the plan.
