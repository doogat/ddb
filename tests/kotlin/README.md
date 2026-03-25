# Doogat DB Kotlin Verification Tests

Minimal Gradle project to verify UniFFI bindings for Android/JVM.

## Prerequisites

See [FFI docs](../../docs/src/technical/ffi.md#prerequisites) for full setup instructions.

Quick summary:
- JDK (`brew install openjdk`)
- Kotlin (`brew install kotlin`)
- cargo-ndk (`cargo install cargo-ndk`)
- Android NDK (`brew install --cask android-ndk`, set `ANDROID_NDK_HOME`)
- Rust targets: `rustup target add aarch64-linux-android x86_64-linux-android`

## Setup

1. Build native library (JVM host testing):

```bash
cargo build -p ddb-core --release
```

2. Generate Kotlin bindings:

```bash
cargo run -p ddb-uniffi-bindgen --bin uniffi-bindgen -- generate \
  --library target/release/libddb_core.dylib \
  --language kotlin --out-dir out/kotlin
```

3. Copy generated bindings:

```bash
cp out/kotlin/**/*.kt tests/kotlin/src/main/kotlin/
```

## Run

JVM (host platform):

```bash
cd tests/kotlin && ./gradlew test
```

Android emulator (requires full AAR build):

```bash
dev/bin/build-android
cd tests/kotlin && ./gradlew connectedAndroidTest
```

Tests use `DoogatDriver.createRepo()` and `registerNode()` directly (no CLI binary needed), making them compatible with Android instrumented test targets.
