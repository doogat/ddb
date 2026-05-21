use ddb_core::app_contract::AppWarning;
use std::io::Write;

pub fn write_warnings(warnings: &[AppWarning], w: &mut impl Write) -> std::io::Result<()> {
    for warning in warnings {
        writeln!(w, "warning: {}: {}", warning.code, warning.message)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_warnings_formats_one_warning_per_line() {
        let warnings = vec![
            AppWarning {
                code: "TITLE_TRUNCATED",
                message: "title was truncated to 255 characters".to_string(),
            },
            AppWarning {
                code: "BACKLINK_SKIPPED",
                message: "source doogat was missing".to_string(),
            },
        ];
        let mut buf = Vec::<u8>::new();

        write_warnings(&warnings, &mut buf).unwrap();

        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "warning: TITLE_TRUNCATED: title was truncated to 255 characters\n\
             warning: BACKLINK_SKIPPED: source doogat was missing\n"
        );
    }
}
