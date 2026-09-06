# Contributing to Doogat DB

Contributions are welcome — bug fixes, documentation improvements, and test coverage especially.

For large features or architectural changes, open an issue to discuss before starting work.

## Dev Setup

1. Install [Rust](https://rustup.rs/) (stable toolchain)
2. Clone the repo and configure git hooks:

```bash
git clone https://github.com/doogat/ddb.git
cd ddb
git config core.hooksPath dev/hooks
```

3. Build and test:

```bash
cargo build
cargo test-ci
```

See [AGENTS.md](AGENTS.md) for the full command reference.

## Coding Standards

Follow the conventions in [AGENTS.md](AGENTS.md). Key points:

- All modules return `error::Result<T>`
- Conventional commit messages: `<type>(<scope>): <description>`
- No `#[allow(...)]` or equivalent warning suppressions — fix root causes
- No stubs, placeholders, or TODO comments

## Testing

Every change needs tests:

- **Unit tests** in the module
- **Integration/e2e tests** in `tests/e2e/` for behavior changes
- **Smoke/integration scenario authoring** — if the PRD adds a CLI command or user-facing behavior, add a `smoke_` scenario under `tests/e2e/`. If it adds a server endpoint, sync behavior, CRDT logic, or a multi-step workflow, add an `integration_` scenario there. Register new modules in `tests/e2e/main.rs`. Execution of these Rust e2e scenarios is delegated to CI (Tier 2).

Run the Tier 1 local gate after every task:

```bash
cargo build
cargo clippy --workspace --all-targets
cargo test-ci
```

Tier 2 runs in CI: full workspace tests, Rust e2e scenarios, property tests, cross-platform validation, and coverage. Do not duplicate the heavy battery locally. The narrow deletion safety exception in `CLAUDE.md` requires Claude-routed sessions to run `cargo test -p ddb-e2e` after tasks that delete or replace existing code paths; purely additive tasks skip that run. See [AGENTS.md](AGENTS.md) for the full two-tier policy.

## Pull Request Process

1. Fork the repo and create a branch from `master`
2. Make your changes with tests
3. Ensure the Tier 1 local gate and required CI checks pass
4. Submit a PR against `master`

Keep PRs focused — one concern per PR.

## Changelog

Every PR with user-facing changes must add an entry under `## [Unreleased]` in CHANGELOG.md. Use the standard sections: Added, Changed, Deprecated, Removed, Fixed, Security. Breaking changes must include migration notes. FFI/UniFFI binding changes must always be noted explicitly. Not enforced by CI — enforced by code review convention.

## Architecture

See [docs/src/](docs/src/) for architecture documentation, module boundaries, and design decisions.

## License

By contributing, you agree that your contributions will be licensed under the [BSL-1.1](LICENSE).
