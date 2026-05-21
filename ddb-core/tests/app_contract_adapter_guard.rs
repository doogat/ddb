use std::fs;
use std::path::Path;
use walkdir::WalkDir;

const FORBIDDEN_CRATES: &[&str] = &[
    "rusqlite::",
    "git2::",
    "redb::",
    "axum::",
    "async_graphql::",
    "uniffi::",
    "uniffi_macros::",
    "extern crate rusqlite",
    "extern crate git2",
    "extern crate redb",
    "extern crate axum",
    "extern crate async_graphql",
    "extern crate uniffi",
    "extern crate uniffi_macros",
];

/// Strips a `//` line comment off a single source line, ignoring `//` that
/// appears inside a double-quoted string. **Scope is intentionally narrow:**
/// block comments (`/* ... */`) and raw string literals (`r#"..."#`) are NOT
/// stripped. App-contract sources today use neither for adapter-crate tokens,
/// so the guard is safe-by-construction; the `guard_does_not_strip_block_comments_or_raw_strings`
/// test below pins the scope. If app-contract style ever changes to use block
/// comments or raw strings containing adapter-crate names, replace that
/// negative test with a positive test and extend this function.
fn strip_line_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut chars = line.char_indices().peekable();
    while let Some((i, ch)) = chars.next() {
        match ch {
            '"' => in_string = !in_string,
            '/' if !in_string => {
                if let Some(&(_, '/')) = chars.peek() {
                    return &line[..i];
                }
            }
            '\\' if in_string => {
                chars.next();
            }
            _ => {}
        }
    }
    line
}

fn forbidden_tokens_in_source(source: &str) -> Vec<&'static str> {
    let mut found = Vec::new();
    for &token in FORBIDDEN_CRATES {
        let token_present = source.lines().any(|line| {
            let code = strip_line_comment(line);
            code.contains(token)
        });
        if token_present {
            found.push(token);
        }
    }
    found
}

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

        for token in forbidden_tokens_in_source(&source) {
            violations.push(format!(
                "{}: contains forbidden import `{}`",
                path.display(),
                token
            ));
        }
    }

    assert!(
        violations.is_empty(),
        "adapter-neutrality violated in ddb-core/src/app_contract/:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn guard_ignores_forbidden_tokens_in_line_comments() {
    let source = "\
// uniffi::Object is documented here
// git2::Repository is also just a reference
// uniffi_macros::export is a decorator
// rusqlite::Connection example
// axum::Router example
fn real_code() {}
";
    assert!(
        forbidden_tokens_in_source(source).is_empty(),
        "guard falsely flagged tokens inside line comments"
    );
}

#[test]
fn guard_detects_uniffi_use_statement() {
    let source = "use uniffi::Object;\n";
    let found = forbidden_tokens_in_source(source);
    assert!(
        found.contains(&"uniffi::"),
        "expected `uniffi::` to be detected, got: {:?}",
        found
    );
}

#[test]
fn guard_detects_uniffi_macros_use_statement() {
    let source = "use uniffi_macros::Object;\n";
    let found = forbidden_tokens_in_source(source);
    assert!(
        found.contains(&"uniffi_macros::"),
        "expected `uniffi_macros::` to be detected, got: {:?}",
        found
    );
}

#[test]
fn guard_detects_git2_use_statement() {
    let source = "use git2::Repository;\n";
    let found = forbidden_tokens_in_source(source);
    assert!(
        found.contains(&"git2::"),
        "expected `git2::` to be detected, got: {:?}",
        found
    );
}

#[test]
fn guard_does_not_strip_block_comments_or_raw_strings() {
    // Documents the current scope: block-comment and raw-string content is NOT
    // stripped. The forbidden tokens here would be flagged by the guard if they
    // appeared in real app_contract source — which is what we want as long as
    // those forms aren't allowed in app_contract. If app_contract style ever
    // changes to use block comments or raw strings that mention adapter crates,
    // replace this test with a positive test that DOES strip them and extend
    // `strip_line_comment` accordingly.
    let source = "/* uniffi::Object example */\nlet s = r#\"uniffi::Object\"#;\n";
    let found = forbidden_tokens_in_source(source);
    assert!(
        !found.is_empty(),
        "guard intentionally does not strip block comments or raw strings; \
         `uniffi::` should have been detected in either the block comment or the raw string"
    );
}
