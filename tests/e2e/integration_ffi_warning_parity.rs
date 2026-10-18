//! Warning parity between the FFI driver and the sibling transports (PRD 00177).
//!
//! Each side runs the same fixture in its own fresh temp repo, then the test
//! compares the emitted warning `code` AND `message`. Both sides are pinned to
//! a non-empty warning list first: a comparison satisfied by two empty lists
//! would still pass on a build that stopped warning altogether.

use crate::common::{DdbTestRepo, ServerGuard};
use ddb_core::ffi::DoogatDriver;
use ddb_core::types::ConflictAction;

const CREATE_TITLE_TEMPLATE_TABLE: &str = "CREATE TABLE ffi_parity_warn (note TEXT)";
const SET_TITLE_TEMPLATE: &str = "ALTER TABLE ffi_parity_warn SET TITLE TEMPLATE 'auto-title'";

/// A fresh repo carrying the `ffi_parity_warn` typedef with a `title_template`.
/// Creating into it with the title omitted is what makes the service emit
/// `TITLE_FROM_TEMPLATE`.
fn repo_with_title_template() -> DdbTestRepo {
    let repo = DdbTestRepo::init();
    repo.ddb()
        .args(["query", CREATE_TITLE_TEMPLATE_TABLE])
        .assert()
        .success();
    repo.ddb()
        .args(["query", SET_TITLE_TEMPLATE])
        .assert()
        .success();
    repo
}

/// `(code, message)` of the single warning `create_structured` emits in `repo`.
fn ffi_create_warning(
    repo: &DdbTestRepo,
    title: Option<&str>,
    doogat_type: &str,
) -> (String, String) {
    let driver =
        DoogatDriver::new(repo.path().display().to_string()).expect("FFI driver must open the repo");
    let result = driver
        .create_structured(
            title.map(str::to_string),
            vec![],
            Some(doogat_type.to_string()),
            None,
            vec![],
            // `ConflictActionRecord` is not re-exported from `ddb_core::ffi`,
            // so it is unnameable here; its public `From<ConflictAction>` impl
            // builds the same value.
            ConflictAction::Error.into(),
        )
        .expect("FFI create_structured must succeed");
    assert_eq!(
        result.warnings.len(),
        1,
        "FFI create_structured must emit exactly one warning, got: {:?}",
        result.warnings
    );
    let warning = &result.warnings[0];
    (warning.code.clone(), warning.message.clone())
}

/// PRD 00177: the FFI driver and GraphQL must render the SAME warning for the
/// same create, down to the message, because both read it off the one app
/// contract output.
#[test]
fn ffi_and_graphql_report_identical_title_from_template_warning() {
    let ffi_repo = repo_with_title_template();
    let (ffi_code, ffi_message) = ffi_create_warning(&ffi_repo, None, "ffi_parity_warn");
    assert_eq!(
        ffi_code, "TITLE_FROM_TEMPLATE",
        "FFI side must warn TITLE_FROM_TEMPLATE, got: {ffi_code}"
    );

    let gql_repo = repo_with_title_template();
    let server = ServerGuard::start(&gql_repo);
    let response = server
        .graphql(r#"mutation { createDoogat(input: { type: "ffi_parity_warn" }) { id title } }"#);
    assert!(
        response.get("errors").is_none(),
        "GraphQL createDoogat must succeed: {response}"
    );
    let warnings = response["extensions"]["warnings"]
        .as_array()
        .unwrap_or_else(|| panic!("extensions.warnings must be an array: {response}"));
    assert_eq!(
        warnings.len(),
        1,
        "GraphQL side must emit exactly one warning: {response}"
    );
    let gql_code = warnings[0]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("warning must carry a string code: {response}"));
    let gql_message = warnings[0]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("warning must carry a string message: {response}"));

    assert_eq!(
        gql_code, ffi_code,
        "FFI and GraphQL must report the identical warning code"
    );
    assert_eq!(
        gql_message, ffi_message,
        "FFI and GraphQL must report the identical warning message"
    );
}

/// PRD 00177: the FFI driver pins `UnregisteredTypePolicy::BaseOnly`, the same
/// policy the CLI create command applies, so both must emit the same
/// `UNREGISTERED_TYPE_BASE_ONLY` warning for an unregistered type.
#[test]
fn ffi_and_cli_report_identical_unregistered_type_base_only_warning() {
    let ffi_repo = DdbTestRepo::init();
    let (ffi_code, ffi_message) =
        ffi_create_warning(&ffi_repo, Some("Project Alpha"), "ffi_parity_unregistered");
    assert_eq!(
        ffi_code, "UNREGISTERED_TYPE_BASE_ONLY",
        "FFI side must warn UNREGISTERED_TYPE_BASE_ONLY, got: {ffi_code}"
    );

    let cli_repo = DdbTestRepo::init();
    let out = cli_repo
        .ddb()
        .args([
            "create",
            "--title",
            "Project Alpha",
            "--type",
            "ffi_parity_unregistered",
        ])
        .output()
        .expect("ddb create must spawn");
    assert!(
        out.status.success(),
        "ddb create with an unregistered --type must exit 0: stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    // `ddb create` has no structured output mode, so the code and message come
    // off the stderr line the CLI renders as `warning: <CODE>: <MESSAGE>`
    // (`ddb-cli/src/warnings.rs`).
    let stderr = String::from_utf8_lossy(&out.stderr);
    let mut cli_warnings = stderr.lines().filter_map(|line| {
        line.strip_prefix("warning: ")
            .and_then(|rest| rest.split_once(": "))
    });
    let (cli_code, cli_message) = cli_warnings
        .next()
        .unwrap_or_else(|| panic!("CLI must render a warning line on stderr, got: {stderr}"));
    assert!(
        cli_warnings.next().is_none(),
        "CLI must render exactly one warning line, got: {stderr}"
    );

    assert_eq!(
        cli_code, ffi_code,
        "FFI and CLI must report the identical warning code"
    );
    assert_eq!(
        cli_message, ffi_message,
        "FFI and CLI must report the identical warning message"
    );
}
