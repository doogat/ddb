use ddb_core::app_contract::{AppOutput, AppWarning};

#[test]
fn appoutput_with_no_warnings_exposes_value() {
    let out = AppOutput {
        value: String::from("hello"),
        warnings: Vec::new(),
    };
    assert_eq!(out.value, "hello");
    assert!(out.warnings.is_empty());
}

#[test]
fn appoutput_with_one_warning_holds_warning() {
    let w = AppWarning {
        code: "SOME_WARNING",
        message: String::from("something noteworthy"),
    };
    let out = AppOutput {
        value: 42u64,
        warnings: vec![w],
    };
    assert_eq!(out.value, 42u64);
    assert_eq!(out.warnings.len(), 1);
    assert_eq!(out.warnings[0].code, "SOME_WARNING");
    assert_eq!(out.warnings[0].message, "something noteworthy");
}

#[test]
fn appoutput_with_multiple_warnings_preserves_all() {
    let warnings = vec![
        AppWarning { code: "FIRST", message: String::from("first") },
        AppWarning { code: "SECOND", message: String::from("second") },
    ];
    let out = AppOutput {
        value: 0u64,
        warnings,
    };
    assert_eq!(out.warnings.len(), 2);
    assert_eq!(out.warnings[0].code, "FIRST");
    assert_eq!(out.warnings[1].code, "SECOND");
}

#[test]
fn appwarning_holds_code_and_message() {
    let w = AppWarning {
        code: "SCREAMING_SNAKE_CODE",
        message: String::from("human readable detail"),
    };
    assert_eq!(w.code, "SCREAMING_SNAKE_CODE");
    assert_eq!(w.message, "human readable detail");
}

#[test]
fn appoutput_works_with_string_value_type() {
    let out = AppOutput {
        value: String::from("text result"),
        warnings: Vec::new(),
    };
    assert_eq!(out.value, "text result");
}

#[test]
fn appoutput_works_with_u64_value_type() {
    let out = AppOutput {
        value: 99u64,
        warnings: Vec::new(),
    };
    assert_eq!(out.value, 99u64);
}

#[test]
fn appoutput_and_appwarning_are_cloneable() {
    let w = AppWarning {
        code: "CLONE_CHECK",
        message: String::from("cloned"),
    };
    let out = AppOutput {
        value: String::from("original"),
        warnings: vec![w.clone()],
    };
    let out2 = out.clone();
    assert_eq!(out2.value, "original");
    assert_eq!(out2.warnings[0].code, "CLONE_CHECK");
    assert_eq!(out2.warnings[0].message, "cloned");
}
