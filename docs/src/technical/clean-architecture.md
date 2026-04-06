# Clean Architecture in Rust

Clean Architecture's core idea: **dependencies point inward**. Outer layers know about inner layers, never the reverse.

## Layer mapping

| Layer | Doogat DB equivalent | Depends on |
|---|---|---|
| Domain | `types.rs`, `error.rs` — pure data structures, no I/O | Nothing |
| Use cases | Business logic — parsing rules, type inference, conflict resolution | Domain only |
| Adapters | `git_ops/mod.rs`, `indexer.rs`, `crdt_resolver.rs` — external system integration | Domain + traits |
| Entry point | `ddb-cli` — wires everything together | All layers |

## Domain layer

Pure data structures with no I/O dependencies. These types are the lingua franca of the codebase.

```rust
// types.rs — no imports from git2, rusqlite, automerge
pub struct DoogatId(String);
pub struct ParsedDoogat { /* ... */ }
pub struct DoogatMeta { /* ... */ }
```

Rules:
- No filesystem, network, or database calls
- No dependencies on external crates (git2, rusqlite, automerge)
- Testable with zero setup

## Use-case layer

Business logic that operates on domain types. Should be testable without touching disk, git, or SQLite.

Examples in Doogat DB:
- Parser logic (frontmatter extraction, reference detection)
- Type inference rules
- Conflict resolution strategy (which field wins)

These should depend on domain types and trait abstractions, not on concrete adapters.

## Adapter layer

Implements trait abstractions using external systems. Each adapter translates between the domain language and an external dependency.

| Adapter | External dependency | Domain interface |
|---|---|---|
| `git_ops/mod.rs` | `git2` | Doogat storage/retrieval |
| `indexer.rs` | `rusqlite` | Search and query |
| `crdt_resolver.rs` | `automerge` | Conflict resolution state |

## Entry point

`ddb-cli/src/main.rs` constructs concrete implementations and wires them together. This is the only place that knows about all concrete types.

## Clean Code principles in Rust

| Principle | Rust equivalent |
|---|---|
| Small functions | Small functions — same |
| Meaningful names | Module + type + fn naming |
| No hidden side effects | Ownership makes side effects explicit |
| Error handling | `Result<T, E>` everywhere, no panics in library code |
| DRY | Traits, generics, macros (sparingly) |
| Test isolation | Trait-based dependency injection |

## Practical guidelines

1. **Keep I/O at the edges.** Business logic functions take domain types in and return domain types out. Let the caller handle I/O.

2. **No panics in library code.** Use `Result` for all fallible operations. Reserve `panic!`/`unwrap` for genuinely impossible states (and document why).

3. **Trait boundaries between modules.** Modules that talk to external systems should implement traits defined in the domain or use-case layer.

4. **Constructor injection.** Pass dependencies as function parameters or struct fields, not via global state or module-level calls.

5. **Test without I/O.** If a test needs a real git repo or SQLite database, it's an integration test. Unit tests should use mock implementations of trait boundaries.
