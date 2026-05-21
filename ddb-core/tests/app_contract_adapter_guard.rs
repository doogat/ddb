use std::fs;
use std::path::Path;
use walkdir::WalkDir;

const FORBIDDEN_CRATES: &[&str] = &[
    "rusqlite::",
    "git2::",
    "redb::",
    "axum::",
    "async_graphql::",
    "extern crate rusqlite",
    "extern crate git2",
    "extern crate redb",
    "extern crate axum",
    "extern crate async_graphql",
];

#[test]
fn no_forbidden_adapter_imports_in_app_contract_sources() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("app_contract");
    assert!(
        dir.exists(),
        "ddb-core/src/app_contract/ does not exist; scaffold the module first"
    );

    let mut violations: Vec<String> = Vec::new();

    for entry in WalkDir::new(&dir).into_iter() {
        let entry = entry.unwrap_or_else(|err| panic!("failed to walk {}: {}", dir.display(), err));
        if !entry
            .path()
            .extension()
            .map(|ext| ext == "rs")
            .unwrap_or(false)
        {
            continue;
        }

        let path = entry.path();
        let source = fs::read_to_string(path)
            .unwrap_or_else(|err| panic!("failed to read {}: {}", path.display(), err));

        for &forbidden in FORBIDDEN_CRATES {
            if source.contains(forbidden) {
                violations.push(format!(
                    "{}: contains forbidden import `{}`",
                    path.display(),
                    forbidden
                ));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "adapter-neutrality violated in ddb-core/src/app_contract/:\n  {}",
        violations.join("\n  ")
    );
}
