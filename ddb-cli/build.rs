fn main() {
    println!("cargo:rerun-if-env-changed=DDB_BUILD_META");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let meta = std::env::var("DDB_BUILD_META").unwrap_or_default();
    println!("cargo:rustc-env=DDB_VERSION={version}{meta}");
}
