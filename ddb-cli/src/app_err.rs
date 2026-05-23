// IMPLEMENTOR: replace this stub with the real implementation
pub fn format_app_error(app: &ddb_core::app_contract::AppError) -> String {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddb_core::app_contract::{AppError, AppErrorCategory};
    use ddb_core::error::DoogatError;

    #[test]
    fn cli_format_matches_legacy_for_validation() {
        let de = DoogatError::Validation("title required".into());
        let expected = format!("error: {}", de);
        let result =
            format_app_error(&AppError::from(DoogatError::Validation("title required".into())));
        assert_eq!(result, expected);
    }

    #[test]
    fn cli_format_matches_legacy_for_not_found() {
        let de = DoogatError::NotFound("doogat 999 missing".into());
        let expected = format!("error: {}", de);
        let result =
            format_app_error(&AppError::from(DoogatError::NotFound("doogat 999 missing".into())));
        assert_eq!(result, expected);
    }

    #[test]
    fn cli_format_matches_legacy_for_structured_unique_violation() {
        let de = DoogatError::unique_violation("link", ["url"], ["x"]);
        let expected = format!("error: {}", de);
        let result = format_app_error(&AppError::from(DoogatError::unique_violation(
            "link",
            ["url"],
            ["x"],
        )));
        assert_eq!(result, expected);
    }

    #[test]
    fn cli_format_matches_legacy_for_internal_git_error() {
        let de = DoogatError::Git("repo corrupt".into());
        let expected = format!("error: {}", de);
        let result =
            format_app_error(&AppError::from(DoogatError::Git("repo corrupt".into())));
        assert_eq!(result, expected);
    }

    #[test]
    fn cli_format_prefixes_error_label() {
        let app = AppError {
            code: "BAD_REQUEST",
            message: "missing title".into(),
            category: AppErrorCategory::InvalidInput,
            field: None,
            details: vec![],
        };
        assert!(
            format_app_error(&app).starts_with("error: "),
            "output must start with 'error: '"
        );
    }
}
