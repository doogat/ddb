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

use crate::types::ConsistencyWarning;

/// Stable code attached to the summarized reindex-skip warning.
pub const REINDEX_SKIPPED_FILES: &str = "REINDEX_SKIPPED_FILES";

/// Summarize skipped/consistency warnings from an implicit background
/// reindex into at most ONE AppWarning — an incidental discovery during an
/// unrelated write should not flood the verb's response envelope on a
/// large corrupt corpus. The full per-file list is not lost: it is logged
/// (`tracing::warn!` per file, already emitted by `batch_index_changes`/
/// `parallel_parse`) and will be the 00181 doctor surface's job to list.
pub(crate) fn summarize_reindex_warnings(warnings: Vec<ConsistencyWarning>) -> Option<AppWarning> {
    let mut skip_paths: Vec<&str> = Vec::new();
    let mut first_description = None;
    for warning in &warnings {
        let path = match warning {
            ConsistencyWarning::UnreadableFile { path, .. } => path,
            ConsistencyWarning::MalformedYaml { path, .. } => path,
            ConsistencyWarning::CrossZoneDuplicate { .. } | ConsistencyWarning::MissingRequired { .. } => continue,
        };
        if skip_paths.contains(&path.as_str()) {
            continue;
        }
        skip_paths.push(path.as_str());
        if first_description.is_none() {
            first_description = Some(describe_consistency_warning(warning));
        }
    }
    let first_description = first_description?;
    let message = if skip_paths.len() == 1 {
        first_description
    } else {
        format!("{first_description} (+{} more; run `ddb reindex` for the full list)", skip_paths.len() - 1)
    };
    Some(AppWarning { code: REINDEX_SKIPPED_FILES, message })
}

pub(crate) fn describe_consistency_warning(w: &ConsistencyWarning) -> String {
    match w {
        ConsistencyWarning::UnreadableFile { path, error } => format!("{path}: unreadable ({error})"),
        ConsistencyWarning::MalformedYaml { path, error } => format!("{path}: malformed ({error})"),
        ConsistencyWarning::CrossZoneDuplicate { path, key } => format!("{path}: duplicate key '{key}'"),
        ConsistencyWarning::MissingRequired { path, field, .. } => format!("{path}: missing required field '{field}'"),
    }
}

/// Stable per-variant code for the FFI `RebuildWarningRecord` (`ffi/driver.rs`
/// needs a `&'static str` code alongside `describe_consistency_warning`'s
/// human message).
pub(crate) fn describe_consistency_warning_code(w: &ConsistencyWarning) -> &'static str {
    match w {
        ConsistencyWarning::UnreadableFile { .. } => "UNREADABLE_FILE",
        ConsistencyWarning::MalformedYaml { .. } => "MALFORMED_YAML",
        ConsistencyWarning::CrossZoneDuplicate { .. } => "CROSS_ZONE_DUPLICATE",
        ConsistencyWarning::MissingRequired { .. } => "MISSING_REQUIRED",
    }
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
            !app_warning.message.contains("ddb doctor"),
            "a single-warning summary must carry no count suffix at all, got: {}",
            app_warning.message
        );
    }

    #[test]
    fn summarize_reindex_warnings_with_multiple_warnings_appends_remaining_count() {
        let first = malformed_yaml("ddb/20260101000000.md", "invalid frontmatter");
        let first_description = crate::app_contract::describe_consistency_warning(&first);
        let advisory_path = "ddb/20260101000002.md";
        let warnings = vec![
            first,
            unreadable_file("ddb/20260101000001.md", "permission denied"),
            cross_zone_duplicate(advisory_path, "title"),
        ];

        let result = crate::app_contract::summarize_reindex_warnings(warnings);

        let app_warning = result.expect("multiple input warnings must summarize to Some");
        assert_eq!(app_warning.code, crate::app_contract::REINDEX_SKIPPED_FILES);
        assert!(
            app_warning.message.starts_with(&first_description),
            "message must still describe the first skip warning, got: {}",
            app_warning.message
        );
        // Only 2 of the 3 inputs are skip variants (the third is an advisory
        // CrossZoneDuplicate, whose file WAS indexed successfully) -> N-1 = 1
        // "more". A count derived from the raw 3-warning length would wrongly
        // say "+2 more".
        assert!(
            app_warning.message.contains("+1 more"),
            "expected the skip-only deduplicated remainder (+1 more), got: {}",
            app_warning.message
        );
        assert!(
            !app_warning.message.contains(advisory_path),
            "an advisory-only path must never appear in the REINDEX_SKIPPED_FILES message, got: {}",
            app_warning.message
        );
        assert!(
            !app_warning.message.contains("ddb doctor"),
            "message must not point at the unshipped `ddb doctor` command, got: {}",
            app_warning.message
        );
        assert!(
            app_warning.message.contains("ddb reindex"),
            "a message carrying a count suffix must point at the real, shipped `ddb reindex` command, got: {}",
            app_warning.message
        );
    }

    #[test]
    fn summarize_reindex_warnings_with_only_advisory_variants_returns_none() {
        // CrossZoneDuplicate and MissingRequired mean the file WAS indexed
        // successfully; an advisory-only input must never be reported as a
        // REINDEX_SKIPPED_FILES warning.
        let warnings = vec![
            cross_zone_duplicate("ddb/a.md", "title"),
            missing_required("ddb/b.md", "task", "due_date"),
        ];

        let result = crate::app_contract::summarize_reindex_warnings(warnings);

        assert!(
            result.is_none(),
            "advisory-only input must not produce a REINDEX_SKIPPED_FILES warning"
        );
    }

    #[test]
    fn summarize_reindex_warnings_with_duplicate_skip_path_yields_single_file_message_with_no_count_suffix(
    ) {
        // The same file can be reported as a skip by two different sources
        // (e.g. parallel_parse and collect_consistency_warnings). It must
        // still count as a single file.
        let warnings = vec![
            malformed_yaml("ddb/dup.md", "reported by parallel_parse"),
            malformed_yaml("ddb/dup.md", "reported by collect_consistency_warnings"),
        ];

        let result = crate::app_contract::summarize_reindex_warnings(warnings);

        let app_warning = result.expect("a duplicated skip path must still summarize to Some");
        assert_eq!(app_warning.code, crate::app_contract::REINDEX_SKIPPED_FILES);
        assert_eq!(
            app_warning.message.matches("ddb/dup.md").count(),
            1,
            "the same path reported twice must appear exactly once in the message, got: {}",
            app_warning.message
        );
        assert!(
            !app_warning.message.contains("more"),
            "a single distinct skip path must carry no count suffix at all, got: {}",
            app_warning.message
        );
    }

    #[test]
    fn summarize_reindex_warnings_with_duplicate_and_distinct_skip_paths_counts_distinct_remainder_only(
    ) {
        let warnings = vec![
            malformed_yaml("ddb/a.md", "invalid frontmatter (first report)"),
            unreadable_file("ddb/b.md", "permission denied"),
            malformed_yaml("ddb/a.md", "invalid frontmatter (second report)"),
            malformed_yaml("ddb/c.md", "invalid frontmatter"),
        ];

        let result = crate::app_contract::summarize_reindex_warnings(warnings);

        let app_warning = result.expect("skip warnings must summarize to Some");
        assert_eq!(app_warning.code, crate::app_contract::REINDEX_SKIPPED_FILES);
        // 3 distinct skip paths (a, b, c) after deduplicating the repeated
        // "a" entry -> N-1 = 2 "more". An undeduplicated count would say
        // "+3 more" instead.
        assert!(
            app_warning.message.contains("+2 more"),
            "expected the deduplicated remainder (+2 more), got: {}",
            app_warning.message
        );
        assert!(
            !app_warning.message.contains("ddb doctor"),
            "message must not point at the unshipped `ddb doctor` command, got: {}",
            app_warning.message
        );
        assert!(
            app_warning.message.contains("ddb reindex"),
            "a message carrying a count suffix must point at the real, shipped `ddb reindex` command, got: {}",
            app_warning.message
        );
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
