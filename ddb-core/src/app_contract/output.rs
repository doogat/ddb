//! Output envelope types for the application contract layer.
//!
//! `AppOutput<T>` wraps a successful result with adapter-neutral warnings.
//! Transports must surface these warnings; they cannot be silently discarded
//! for promised workflows (PRD 00147).

/// A stable, adapter-neutral warning attached to a successful command result.
///
/// `code` is a stable static string suitable for programmatic handling;
/// `message` is a human-readable description that transports may format for
/// end users.
#[derive(Debug, Clone)]
pub struct AppWarning {
    pub code: &'static str,
    pub message: String,
}

/// Adapter-neutral envelope for a successful application command result.
///
/// The `value` field carries the primary result; `warnings` carries
/// best-effort or partial-success signals that transports must forward to
/// callers rather than discard.
#[derive(Debug, Clone)]
pub struct AppOutput<T> {
    pub value: T,
    pub warnings: Vec<AppWarning>,
}

#[cfg(test)]
mod tests {
    // All references below go through `crate::app_contract::...` (never
    // `super::` / `super::*`) even for items defined in this very file.
    // `output` is a private module (`mod output;` in `app_contract/mod.rs`),
    // so a helper or the `REINDEX_SKIPPED_FILES` const that compiles via
    // `super::` but was never re-exported from `app_contract/mod.rs` would
    // still make these tests fail to compile -- which is the point (see
    // WHAT YOUR TESTS MUST BIND item 6).

    fn malformed_yaml(path: &str, error: &str) -> crate::types::ConsistencyWarning {
        crate::types::ConsistencyWarning::MalformedYaml {
            path: path.to_string(),
            error: error.to_string(),
        }
    }

    fn unreadable_file(path: &str, error: &str) -> crate::types::ConsistencyWarning {
        crate::types::ConsistencyWarning::UnreadableFile {
            path: path.to_string(),
            error: error.to_string(),
        }
    }

    fn cross_zone_duplicate(path: &str, key: &str) -> crate::types::ConsistencyWarning {
        crate::types::ConsistencyWarning::CrossZoneDuplicate {
            path: path.to_string(),
            key: key.to_string(),
        }
    }

    fn missing_required(
        path: &str,
        type_name: &str,
        field: &str,
    ) -> crate::types::ConsistencyWarning {
        crate::types::ConsistencyWarning::MissingRequired {
            path: path.to_string(),
            type_name: type_name.to_string(),
            field: field.to_string(),
        }
    }

    #[test]
    fn summarize_reindex_warnings_returns_none_for_empty_input() {
        let result = crate::app_contract::summarize_reindex_warnings(Vec::new());
        assert!(result.is_none());
    }

    #[test]
    fn reindex_skipped_files_code_is_pinned_to_its_exact_stable_string() {
        // This code ships to FFI consumers via `RebuildWarningRecord.code`,
        // so a silent rename must fail this test.
        assert_eq!(
            crate::app_contract::REINDEX_SKIPPED_FILES,
            "REINDEX_SKIPPED_FILES"
        );
    }

    #[test]
    fn summarize_reindex_warnings_with_one_warning_has_no_count_suffix() {
        let warning = malformed_yaml("ddb/20260101000000.md", "invalid frontmatter");
        let expected_message = crate::app_contract::describe_consistency_warning(&warning);

        let result = crate::app_contract::summarize_reindex_warnings(vec![warning]);

        let app_warning = result.expect("exactly one input warning must summarize to Some");
        assert_eq!(app_warning.code, crate::app_contract::REINDEX_SKIPPED_FILES);
        assert_eq!(app_warning.message, expected_message);
        assert!(
            !app_warning.message.contains("more, see ddb doctor"),
            "a single-warning summary must carry no count suffix at all, got: {}",
            app_warning.message
        );
    }

    #[test]
    fn summarize_reindex_warnings_with_multiple_warnings_appends_remaining_count() {
        let first = malformed_yaml("ddb/20260101000000.md", "invalid frontmatter");
        let first_description = crate::app_contract::describe_consistency_warning(&first);
        let warnings = vec![
            first,
            unreadable_file("ddb/20260101000001.md", "permission denied"),
            cross_zone_duplicate("ddb/20260101000002.md", "title"),
        ];

        let result = crate::app_contract::summarize_reindex_warnings(warnings);

        let app_warning = result.expect("multiple input warnings must summarize to Some");
        assert_eq!(app_warning.code, crate::app_contract::REINDEX_SKIPPED_FILES);
        // 3 input warnings -> N-1 = 2 "more". Asserts the exact count, not
        // just presence of a suffix, so a hardcoded "+1" fails here.
        let expected_message = format!("{} (+2 more, see ddb doctor)", first_description);
        assert_eq!(app_warning.message, expected_message);
    }

    #[test]
    fn describe_consistency_warning_produces_distinct_path_bearing_messages_for_all_variants() {
        let malformed = malformed_yaml("ddb/a.md", "bad yaml");
        let unreadable = unreadable_file("ddb/b.md", "io error");
        let duplicate = cross_zone_duplicate("ddb/c.md", "status");
        let missing = missing_required("ddb/d.md", "task", "due_date");

        let malformed_msg = crate::app_contract::describe_consistency_warning(&malformed);
        let unreadable_msg = crate::app_contract::describe_consistency_warning(&unreadable);
        let duplicate_msg = crate::app_contract::describe_consistency_warning(&duplicate);
        let missing_msg = crate::app_contract::describe_consistency_warning(&missing);

        assert!(malformed_msg.contains("ddb/a.md"));
        assert!(unreadable_msg.contains("ddb/b.md"));
        assert!(duplicate_msg.contains("ddb/c.md"));
        assert!(missing_msg.contains("ddb/d.md"));

        let messages = [
            &malformed_msg,
            &unreadable_msg,
            &duplicate_msg,
            &missing_msg,
        ];
        for i in 0..messages.len() {
            for j in (i + 1)..messages.len() {
                assert_ne!(
                    messages[i], messages[j],
                    "each ConsistencyWarning variant must produce a distinct message; \
                     a swapped or copy-pasted match arm must fail this"
                );
            }
        }
    }

    #[test]
    fn describe_consistency_warning_code_maps_each_variant_to_its_own_distinct_code() {
        let malformed = malformed_yaml("ddb/a.md", "bad yaml");
        let unreadable = unreadable_file("ddb/b.md", "io error");
        let duplicate = cross_zone_duplicate("ddb/c.md", "status");
        let missing = missing_required("ddb/d.md", "task", "due_date");

        let codes = [
            crate::app_contract::describe_consistency_warning_code(&malformed),
            crate::app_contract::describe_consistency_warning_code(&unreadable),
            crate::app_contract::describe_consistency_warning_code(&duplicate),
            crate::app_contract::describe_consistency_warning_code(&missing),
        ];

        // Pin each variant's code to its exact stable contract string. These
        // codes ship to FFI consumers via `RebuildWarningRecord.code`, so a
        // silent rename must fail this test (distinctness alone would not
        // catch a rename that keeps all four codes different from each
        // other).
        assert_eq!(codes[0], "MALFORMED_YAML");
        assert_eq!(codes[1], "UNREADABLE_FILE");
        assert_eq!(codes[2], "CROSS_ZONE_DUPLICATE");
        assert_eq!(codes[3], "MISSING_REQUIRED");

        for i in 0..codes.len() {
            for j in (i + 1)..codes.len() {
                assert_ne!(
                    codes[i], codes[j],
                    "each ConsistencyWarning variant must map to its own stable code; \
                     a copy-pasted match arm must fail this"
                );
            }
        }
    }
}
