# Doogat DB Swift Verification Tests

Minimal Swift package to verify UniFFI bindings on Apple platforms.

## Prerequisites

See [FFI docs](../../docs/src/technical/ffi.md#prerequisites) for full setup instructions.

Quick summary:
- Xcode (full install, not just CLT)
- Rust targets: `rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios aarch64-apple-darwin`

## Setup

1. Build the XCFramework from the repo root:

```bash
dev/bin/build-xcframework
```

2. Copy generated bindings into the source target:

```bash
cp out/swift/ddb_core.swift tests/swift/Sources/DoogatDB/
```

## Run

```bash
cd tests/swift && swift test
```

Tests use `DoogatDriver.createRepo()` and `registerNode()` directly (no CLI binary needed), making them compatible with iOS simulator targets.
