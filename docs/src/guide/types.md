# Type Definitions

Doogat DB supports typed doogats. A doogat's `type` field determines which materialized SQLite table it belongs to. Type schemas can be explicit (`_typedef` doogats) or inferred from data.

## How It Works

On `ddb reindex`, the indexer:

1. Indexes all doogats
2. Finds all distinct `type` values
3. For each type: loads the `_typedef` (if any), infers schema from data, merges them
4. Creates a SQLite table with the merged columns and populates it

## Explicit Types (_typedef)

A `_typedef` doogat defines a table schema explicitly:

```yaml
---
id: 20260226143000
title: project
type: _typedef
columns:
  - name: completed
    data_type: BOOLEAN
    zone: frontmatter
  - name: deliverable
    data_type: TEXT
    zone: frontmatter
  - name: parent
    data_type: TEXT
    zone: reference
    references: project
crdt_strategy: preset:append-log
template_sections:
  - Description
  - Log
---
```

Column properties:
- `data_type`: TEXT, INTEGER, REAL, BOOLEAN
- `zone`: frontmatter, body, reference
- `required`: if true, missing values produce consistency warnings
- `references`: FK target type name
- `search_boost`: FTS weight -- boosts search ranking for matches in this type's frontmatter fields via bm25() (e.g. `search_boost: 1.5` on a contact's `email` column makes email matches rank higher)
- `allowed_values`: list of valid values (enum constraint); generates CHECK in SQLite
- `default_value`: default value filled on INSERT when column is omitted

## Implicit Types (Inferred)

Types without a `_typedef` are inferred from data. The indexer scans all doogats of that type and infers columns from:

- **Frontmatter** extra keys → column type inferred (integer → float → boolean → text)
- **Body** `## headings` → TEXT columns in body zone
- **Reference** `key:: value` fields → TEXT columns in reference zone

Inferred-only types are tracked internally and reported in the rebuild summary.

## Merged Schemas

When both a `_typedef` and data exist, schemas are merged:
- `_typedef` columns take precedence (type, zone, required flags)
- Inferred columns not in the `_typedef` are appended

## Bundled Types

Doogat DB ships five built-in type definitions:

### project

Columns: `completed` (BOOLEAN), `deliverable` (TEXT), `parent` (FK→project), `ticket` (TEXT), `us` (TEXT)

CRDT: `preset:append-log` | Sections: Description, Log, Plan, Solution

### contact

Columns: `aliases` (TEXT), `contact-type` (TEXT), `email` (TEXT, boost 1.5)

CRDT: `preset:default` | Sections: First contact, Timeline, Relationships

### literature-note

Columns: `author` (TEXT), `source` (TEXT), `year` (INTEGER), `url` (TEXT)

CRDT: `preset:default` | Sections: Summary, Key Arguments, Quotes, Personal Response

### meeting-minutes

Columns: `date` (TEXT), `attendees` (TEXT), `location` (TEXT)

CRDT: `preset:append-log` | Sections: Agenda, Log, Decisions, Action Items

### kanban

Columns: `status` (TEXT, enum: backlog/todo/doing/done/blocked, default: backlog), `priority` (TEXT, enum: low/medium/high/critical), `assignee` (TEXT), `due` (TEXT), `parent` (FK→kanban)

CRDT: `preset:last-writer-wins` | Sections: Description, Acceptance Criteria

## CLI Commands

### Install a bundled type

```bash
ddb type install project
ddb type install contact
ddb type install literature-note
ddb type install meeting-minutes
ddb type install kanban
```

Writes the `_typedef` doogat to `ddb/_typedef/`, commits, and indexes.

### Suggest a typedef from data

```bash
ddb type suggest mytype
```

Infers a schema from existing doogats with `type: mytype` and prints a `_typedef` doogat to stdout. Redirect to a file and commit to make it permanent.

## SQL Access

Typed doogats are queryable via SQL:

```bash
ddb query "SELECT id, completed, deliverable FROM project WHERE completed = 0"
ddb query "INSERT INTO project (deliverable, completed) VALUES ('Ship v1', 0)"
```

See [Search & Queries](./search.md) for more SQL examples.
