use ddb_cli::commands::crud::write_warnings;
use ddb_core::app_contract::AppWarning;

#[test]
fn write_warnings_formats_one_warning_per_line() {
    let warnings = vec![AppWarning {
        code: "TITLE_TRUNCATED",
        message: "title was truncated to 255 characters".to_string(),
    }];
    let mut buf = Vec::<u8>::new();
    write_warnings(&warnings, &mut buf).unwrap();
    let output = String::from_utf8(buf).unwrap();
    assert!(
        output.contains("TITLE_TRUNCATED"),
        "output must contain warning code; got: {output:?}"
    );
    assert!(
        output.contains("title was truncated to 255 characters"),
        "output must contain warning message; got: {output:?}"
    );
}
