# Contributing to ZettelDB

Contributions are welcome — bug fixes, documentation improvements, and test coverage especially.

For large features or architectural changes, open an issue to discuss before starting work.

## Dev Setup

1. Install [Rust](https://rustup.rs/) (stable toolchain)
2. Clone the repo and configure git hooks:

```bash
git clone https://github.com/doogat/zdb.git
cd zdb
git config core.hooksPath dev/hooks
```

3. Build and test:

```bash
cargo build
cargo test
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
- **Integration/e2e tests** in `tests/` for behavior changes
- **Smoke test** scenarios in `tests/smoke.sh` and `tests/smoke.ps1` for CLI or server changes

Run the full suite before submitting:

```bash
cargo clippy --workspace
cargo test --workspace
```

## Pull Request Process

1. Fork the repo and create a branch from `master`
2. Make your changes with tests
3. Ensure `cargo clippy --workspace` and `cargo test --workspace` pass
4. Submit a PR against `master`

Keep PRs focused — one concern per PR.

## Changelog

Every PR with user-facing changes must add an entry under `## [Unreleased]` in CHANGELOG.md. Use the standard sections: Added, Changed, Deprecated, Removed, Fixed, Security. Breaking changes must include migration notes. FFI/UniFFI binding changes must always be noted explicitly. Not enforced by CI — enforced by code review convention.

## Architecture

See [docs/src/](docs/src/) for architecture documentation, module boundaries, and design decisions.

## License

By contributing, you agree that your contributions will be licensed under the [BSL-1.1](LICENSE).
