use std::collections::BTreeMap;

use super::result::ConformanceResult;

/// Classified difference between two driver results. Maps to the six
/// categories PRD 00148 lists for the "Difference classifier" capability,
/// plus `Match` (identical) and `VariantMismatch` (catch-all for incompatible
/// variant pairs like `Ok` vs `Err`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffClass {
    /// `ConformanceResult` PartialEq holds — results are identical.
    Match,
    /// Both `Ok`, but `value` fields differ (warnings may also differ; the
    /// value diff dominates the classification).
    ValueMismatch,
    /// Both `Ok`, `value` matches, but `warnings` differ.
    WarningMismatch,
    /// Both `Err`, but the `ConformanceError` shapes (code/message/context)
    /// differ.
    ErrorMismatch,
    /// At least one driver returned `Unsupported`. Either both are
    /// `Unsupported` with different reasons, or one driver said the operation
    /// was unsupported while the other did not.
    UnsupportedOperation,
    /// At least one driver returned `SetupFailed`. Either both reported a
    /// setup failure with different reasons, or one driver failed setup while
    /// the other ran the operation.
    SetupFailure,
    /// `compare_per_step` only: one slice ran out of results before the
    /// other. The unmatched trailing entries are reported as `MissingField`
    /// so a length divergence between drivers is not silently dropped.
    MissingField,
    /// Catch-all for variant pairs not covered by the more specific
    /// categories above (most commonly `Ok` vs `Err`).
    VariantMismatch,
}

pub fn compare(left: &ConformanceResult, right: &ConformanceResult) -> DiffClass {
    if left == right {
        return DiffClass::Match;
    }
    match (left, right) {
        (
            ConformanceResult::Ok {
                value: lv,
                warnings: lw,
            },
            ConformanceResult::Ok {
                value: rv,
                warnings: rw,
            },
        ) => {
            if lv != rv {
                DiffClass::ValueMismatch
            } else if lw != rw {
                DiffClass::WarningMismatch
            } else {
                // PartialEq above was false yet all fields match — unreachable
                // in practice; classify defensively as Match.
                DiffClass::Match
            }
        }
        (ConformanceResult::Err(_), ConformanceResult::Err(_)) => DiffClass::ErrorMismatch,
        (ConformanceResult::Unsupported { .. }, _) | (_, ConformanceResult::Unsupported { .. }) => {
            DiffClass::UnsupportedOperation
        }
        (ConformanceResult::SetupFailed { .. }, _) | (_, ConformanceResult::SetupFailed { .. }) => {
            DiffClass::SetupFailure
        }
        // Everything else is an incompatible variant pair (e.g. Ok vs Err).
        _ => DiffClass::VariantMismatch,
    }
}

pub fn compare_per_step(left: &[ConformanceResult], right: &[ConformanceResult]) -> Vec<DiffClass> {
    let max_len = left.len().max(right.len());
    let mut result = Vec::with_capacity(max_len);
    for i in 0..max_len {
        match (left.get(i), right.get(i)) {
            (Some(l), Some(r)) => result.push(compare(l, r)),
            // One slice ran out of results — surface the divergence rather
            // than silently truncating (PRD 00148 cycle-2 F9).
            (Some(_), None) | (None, Some(_)) => result.push(DiffClass::MissingField),
            (None, None) => unreachable!("max_len bounds prevent both being None"),
        }
    }
    result
}

/// One search hit carrying all ten core `ddb_core::types::SearchResult`
/// fields, decoded from either driver's transport output (CLI text or
/// GraphQL JSON) so search results can be compared field by field instead of
/// as opaque, format-specific strings.
///
/// Normalizations shared by both decoders, so the two transports' encodings
/// of the same core value decode equal: an empty `updated_at` is `""` (GraphQL
/// sends `null`), and an empty `fields` map is `None` (the CLI prints no field
/// lines for either).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHitShape {
    pub id: String,
    pub title: String,
    pub path: String,
    pub snippet: String,
    pub rank: f64,
    pub updated_at: String,
    pub tags: Vec<String>,
    pub doogat_type: Option<String>,
    pub fields: Option<BTreeMap<String, String>>,
    pub created_at: Option<String>,
}

/// Decode `ddb search` stdout (`ddb-cli/src/commands/query.rs::search`).
///
/// Format: `no results`, or a `Showing a-b of N results` header followed by
/// one block per hit:
///
/// ```text
/// [<id>] <title> (<path>)
///   <snippet>
///   type: <type|->  rank: <rank>  tags: <a, b>
///   created: <created|->  updated: <updated>
///   <field>: <value>        (zero or more)
/// ```
pub fn parse_cli_search_hits(stdout: &str) -> Result<Vec<SearchHitShape>, String> {
    let stdout = stdout.trim();
    if stdout == "no results" {
        return Ok(vec![]);
    }
    let mut lines = stdout.lines().peekable();
    let header = lines.next().unwrap_or_default();
    if !header.starts_with("Showing ") {
        return Err(format!("unexpected CLI search header: {header:?}"));
    }
    let mut hits = Vec::new();
    while let Some(head) = lines.next() {
        hits.push(parse_cli_hit_block(head, &mut lines)?);
    }
    Ok(hits)
}

/// Decode one hit block (header line already consumed into `head`; the
/// snippet/meta/stamps/fields lines that follow are pulled from `lines`).
fn parse_cli_hit_block(
    head: &str,
    lines: &mut std::iter::Peekable<std::str::Lines>,
) -> Result<SearchHitShape, String> {
    let (id, title, path) = parse_cli_hit_head(head)?;
    let snippet = cli_line(lines.next(), "  ", "snippet")?.to_string();

    let meta = cli_line(lines.next(), "  type: ", "type/rank/tags")?;
    let (doogat_type, rest) = split_cli(meta, "  rank: ")?;
    let (rank, tags) = split_cli(rest, "  tags:")?;
    let rank: f64 = rank
        .parse()
        .map_err(|e| format!("CLI rank {rank:?} is not a number: {e}"))?;
    let tags = tags.trim_start();
    let tags: Vec<String> = if tags.is_empty() {
        vec![]
    } else {
        tags.split(", ").map(str::to_string).collect()
    };

    let stamps = cli_line(lines.next(), "  created: ", "created/updated")?;
    let (created_at, updated_at) = split_cli(stamps, "  updated:")?;

    let mut fields = BTreeMap::new();
    while let Some(line) = lines.next_if(|l| l.starts_with("  ")) {
        let (key, value) = split_cli(&line[2..], ":")?;
        fields.insert(
            key.to_string(),
            value.strip_prefix(' ').unwrap_or(value).to_string(),
        );
    }

    Ok(SearchHitShape {
        id,
        title,
        path,
        snippet,
        rank,
        updated_at: updated_at.trim_start().to_string(),
        tags,
        doogat_type: cli_dash_none(doogat_type),
        fields: (!fields.is_empty()).then_some(fields),
        created_at: cli_dash_none(created_at),
    })
}

fn parse_cli_hit_head(head: &str) -> Result<(String, String, String), String> {
    let malformed = || format!("malformed CLI hit header: {head:?}");
    let (id, rest) = head
        .strip_prefix('[')
        .and_then(|s| s.split_once("] "))
        .ok_or_else(malformed)?;
    let (title, path) = rest
        .strip_suffix(')')
        .and_then(|s| s.rsplit_once(" ("))
        .ok_or_else(malformed)?;
    Ok((id.to_string(), title.to_string(), path.to_string()))
}

fn cli_line<'a>(line: Option<&'a str>, prefix: &str, what: &str) -> Result<&'a str, String> {
    let line = line.ok_or_else(|| format!("CLI search output ended before the {what} line"))?;
    line.strip_prefix(prefix)
        .ok_or_else(|| format!("CLI {what} line {line:?} does not start with {prefix:?}"))
}

fn split_cli<'a>(s: &'a str, sep: &str) -> Result<(&'a str, &'a str), String> {
    s.split_once(sep)
        .ok_or_else(|| format!("CLI search line {s:?} is missing {sep:?}"))
}

fn cli_dash_none(s: &str) -> Option<String> {
    (s != "-").then(|| s.to_string())
}

/// Decode the GraphQL driver's search value: the JSON `hits` array selected
/// with all ten `SearchHit` fields. A hit missing any of the ten keys is an
/// error, so the full field set is asserted present, not just compared.
pub fn parse_graphql_search_hits(json: &str) -> Result<Vec<SearchHitShape>, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("GraphQL search value is not JSON: {e}: {json}"))?;
    let hits = value
        .as_array()
        .ok_or_else(|| format!("GraphQL search value is not an array: {json}"))?;
    hits.iter().map(parse_graphql_hit).collect()
}

fn parse_graphql_hit(hit: &serde_json::Value) -> Result<SearchHitShape, String> {
    let rank = gql_field(hit, "rank")?
        .as_f64()
        .ok_or_else(|| format!("GraphQL hit `rank` is not a number: {hit}"))?;
    let tags = gql_field(hit, "tags")?
        .as_array()
        .ok_or_else(|| format!("GraphQL hit `tags` is not an array: {hit}"))?
        .iter()
        .map(|t| t.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| format!("GraphQL hit `tags` has a non-string entry: {hit}"))?;
    let fields = match gql_field(hit, "fields")? {
        serde_json::Value::Null => None,
        serde_json::Value::Object(map) => {
            let decoded = map
                .iter()
                .map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect::<Option<BTreeMap<_, _>>>()
                .ok_or_else(|| format!("GraphQL hit `fields` has a non-string value: {hit}"))?;
            (!decoded.is_empty()).then_some(decoded)
        }
        other => return Err(format!("GraphQL hit `fields` is not an object: {other}")),
    };
    Ok(SearchHitShape {
        id: gql_string(hit, "id")?,
        title: gql_string(hit, "title")?,
        path: gql_string(hit, "path")?,
        snippet: gql_string(hit, "snippet")?,
        rank,
        updated_at: gql_opt_string(hit, "updated_at")?.unwrap_or_default(),
        tags,
        doogat_type: gql_opt_string(hit, "type")?,
        fields,
        created_at: gql_opt_string(hit, "created_at")?,
    })
}

fn gql_field<'a>(hit: &'a serde_json::Value, name: &str) -> Result<&'a serde_json::Value, String> {
    hit.get(name)
        .ok_or_else(|| format!("GraphQL hit is missing `{name}`: {hit}"))
}

fn gql_string(hit: &serde_json::Value, name: &str) -> Result<String, String> {
    gql_field(hit, name)?
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| format!("GraphQL hit `{name}` is not a string: {hit}"))
}

fn gql_opt_string(hit: &serde_json::Value, name: &str) -> Result<Option<String>, String> {
    match gql_field(hit, name)? {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(s) => Ok(Some(s.clone())),
        other => Err(format!("GraphQL hit `{name}` is not a string or null: {other}")),
    }
}

/// Strip the values that legitimately differ between drivers because each
/// driver runs in its own repo, so the remaining fields can be compared with
/// plain equality:
///
/// - `id` becomes `$<step>`, the index of the `CreateDoogat` step that
///   returned it (`created` pairs step index with that driver's id). A hit
///   no step created is an error: the search returned a stray doogat.
/// - `path` has that id replaced by the same `$<step>` token.
/// - `updated_at` / `created_at` become a presence marker: wall-clock
///   timestamps differ across repos, but whether each is set must not.
///
/// The result is sorted by the `$<step>` token so hit order (rank ties) does
/// not decide equality.
pub fn anonymize_search_hits(
    hits: &[SearchHitShape],
    created: &[(usize, String)],
) -> Result<Vec<SearchHitShape>, String> {
    let mut anonymized = hits
        .iter()
        .map(|hit| {
            let (step, _) = created
                .iter()
                .find(|(_, id)| *id == hit.id)
                .ok_or_else(|| {
                    format!(
                        "hit {} ({:?}) was not created by this workflow",
                        hit.id, hit.title
                    )
                })?;
            let token = format!("${step}");
            Ok(SearchHitShape {
                path: hit.path.replace(&hit.id, &token),
                id: token,
                updated_at: presence_marker(&hit.updated_at),
                created_at: hit.created_at.as_deref().map(presence_marker),
                ..hit.clone()
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    anonymized.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(anonymized)
}

/// Relative tolerance for comparing BM25 `rank` across drivers. Each driver
/// indexes its own repo, and the same corpus scores within a few ULPs there,
/// not bit-identically (observed: 1-ULP drift on a -1.098e-6 rank).
const RANK_RELATIVE_TOLERANCE: f64 = 1e-9;

/// Field-by-field equality for two (anonymized) hit lists: every field must
/// be equal exactly, except `rank`, which must agree within
/// `RANK_RELATIVE_TOLERANCE`.
pub fn search_hits_match(left: &[SearchHitShape], right: &[SearchHitShape]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(l, r)| {
            ranks_match(l.rank, r.rank)
                && SearchHitShape {
                    rank: r.rank,
                    ..l.clone()
                } == *r
        })
}

fn ranks_match(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() <= RANK_RELATIVE_TOLERANCE * a.abs().max(b.abs())
}

fn presence_marker(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        "<present>".into()
    }
}

#[cfg(test)]
mod tests {
    use super::super::result::{
        ConformanceError, ConformanceResult, ConformanceValue, ConformanceWarning,
    };
    use super::*;

    // --- search hit decoding ---

    const CLI_TWO_HITS: &str = "Showing 1-2 of 2 results\n\
        [20260101000002] Thai soup (ddb/20260101000002.md)\n\
        \x20 <b>filterterm</b> soup\n\
        \x20 type: recipe  rank: -0.5  tags: dinner\n\
        \x20 created: 2026-01-01  updated: 2026-01-01T00:00:02Z\n\
        \x20 cuisine: thai\n\
        [20260101000004] Plain note (ddb/20260101000004.md)\n\
        \x20 <b>filterterm</b> note\n\
        \x20 type: -  rank: -0.25  tags: \n\
        \x20 created: -  updated:";

    fn soup_hit() -> SearchHitShape {
        SearchHitShape {
            id: "20260101000002".into(),
            title: "Thai soup".into(),
            path: "ddb/20260101000002.md".into(),
            snippet: "<b>filterterm</b> soup".into(),
            rank: -0.5,
            updated_at: "2026-01-01T00:00:02Z".into(),
            tags: vec!["dinner".into()],
            doogat_type: Some("recipe".into()),
            fields: Some(BTreeMap::from([("cuisine".into(), "thai".into())])),
            created_at: Some("2026-01-01".into()),
        }
    }

    fn note_hit() -> SearchHitShape {
        SearchHitShape {
            id: "20260101000004".into(),
            title: "Plain note".into(),
            path: "ddb/20260101000004.md".into(),
            snippet: "<b>filterterm</b> note".into(),
            rank: -0.25,
            updated_at: String::new(),
            tags: vec![],
            doogat_type: None,
            fields: None,
            created_at: None,
        }
    }

    #[test]
    fn cli_search_output_decodes_all_ten_fields_per_hit() {
        let hits = parse_cli_search_hits(CLI_TWO_HITS).expect("CLI output must decode");
        assert_eq!(hits, vec![soup_hit(), note_hit()]);
    }

    #[test]
    fn cli_no_results_decodes_to_empty() {
        assert_eq!(parse_cli_search_hits("no results"), Ok(vec![]));
    }

    #[test]
    fn cli_output_with_truncated_hit_block_is_an_error() {
        let truncated = "Showing 1-1 of 1 results\n[1] T (ddb/1.md)\n  snippet";
        assert!(parse_cli_search_hits(truncated).is_err());
    }

    #[test]
    fn graphql_hits_decode_equal_to_cli_rendering_of_same_core_values() {
        let json = serde_json::json!([
            {
                "id": "20260101000002", "title": "Thai soup",
                "path": "ddb/20260101000002.md", "snippet": "<b>filterterm</b> soup",
                "rank": -0.5, "updated_at": "2026-01-01T00:00:02Z", "tags": ["dinner"],
                "type": "recipe", "fields": {"cuisine": "thai"}, "created_at": "2026-01-01"
            },
            {
                "id": "20260101000004", "title": "Plain note",
                "path": "ddb/20260101000004.md", "snippet": "<b>filterterm</b> note",
                "rank": -0.25, "updated_at": null, "tags": [],
                "type": null, "fields": {}, "created_at": null
            }
        ])
        .to_string();
        let graphql = parse_graphql_search_hits(&json).expect("GraphQL hits must decode");
        let cli = parse_cli_search_hits(CLI_TWO_HITS).expect("CLI output must decode");
        assert_eq!(graphql, cli);
    }

    #[test]
    fn graphql_hit_missing_any_core_field_is_an_error() {
        let full = serde_json::json!({
            "id": "1", "title": "T", "path": "ddb/1.md", "snippet": "", "rank": 0.0,
            "updated_at": null, "tags": [], "type": null, "fields": null, "created_at": null
        });
        assert!(parse_graphql_search_hits(&serde_json::json!([full.clone()]).to_string()).is_ok());
        for key in full.as_object().unwrap().keys() {
            let mut partial = full.clone();
            partial.as_object_mut().unwrap().remove(key);
            let json = serde_json::json!([partial]).to_string();
            assert!(
                parse_graphql_search_hits(&json).is_err(),
                "a hit without `{key}` must not decode"
            );
        }
    }

    #[test]
    fn anonymize_maps_ids_to_creating_step_and_masks_timestamps() {
        let created = vec![(2, "20260101000002".to_string()), (4, "20260101000004".to_string())];
        let anonymized =
            anonymize_search_hits(&[note_hit(), soup_hit()], &created).expect("known ids");
        assert_eq!(anonymized[0].id, "$2");
        assert_eq!(anonymized[0].path, "ddb/$2.md");
        assert_eq!(anonymized[0].updated_at, "<present>");
        assert_eq!(anonymized[0].created_at.as_deref(), Some("<present>"));
        assert_eq!(anonymized[1].id, "$4");
        assert_eq!(anonymized[1].updated_at, "");
        assert_eq!(anonymized[1].created_at, None);
        assert_eq!(anonymized[1].title, "Plain note");
    }

    #[test]
    fn hits_match_despite_one_ulp_rank_drift() {
        let left = soup_hit();
        let right = SearchHitShape {
            rank: f64::from_bits(left.rank.to_bits() + 1),
            ..soup_hit()
        };
        assert_ne!(left.rank, right.rank);
        assert!(search_hits_match(&[left], &[right]));
    }

    #[test]
    fn hits_do_not_match_on_real_rank_or_field_difference() {
        let rank_off = SearchHitShape {
            rank: -0.6,
            ..soup_hit()
        };
        assert!(!search_hits_match(&[soup_hit()], &[rank_off]));
        let tags_off = SearchHitShape {
            tags: vec!["lunch".into()],
            ..soup_hit()
        };
        assert!(!search_hits_match(&[soup_hit()], &[tags_off]));
        assert!(!search_hits_match(&[soup_hit()], &[soup_hit(), note_hit()]));
    }

    #[test]
    fn anonymize_rejects_hit_the_workflow_did_not_create() {
        let created = vec![(2, "20260101000002".to_string())];
        assert!(anonymize_search_hits(&[note_hit()], &created).is_err());
    }

    // --- compare: DiffClass::Match ---

    #[test]
    fn match_when_both_ok_identical() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Int(42),
            warnings: vec![],
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_ok_with_warnings_identical() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::String("hello".into()),
            warnings: vec![ConformanceWarning {
                code: "W001".into(),
                message: "deprecated".into(),
                fields: serde_json::Map::new(),
            }],
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_err_identical() {
        let a = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "not found".into(),
            context: serde_json::Map::new(),
        });
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_unsupported_identical() {
        let a = ConformanceResult::Unsupported {
            reason: "not implemented".into(),
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    #[test]
    fn match_when_both_setup_failed_identical() {
        let a = ConformanceResult::SetupFailed {
            reason: "db unavailable".into(),
        };
        let b = a.clone();
        assert_eq!(compare(&a, &b), DiffClass::Match);
    }

    // --- compare: ValueMismatch / WarningMismatch ---

    #[test]
    fn value_mismatch_when_both_ok_with_different_values() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Int(1),
            warnings: vec![],
        };
        let b = ConformanceResult::Ok {
            value: ConformanceValue::Int(2),
            warnings: vec![],
        };
        assert_eq!(compare(&a, &b), DiffClass::ValueMismatch);
    }

    #[test]
    fn warning_mismatch_when_value_matches_but_warnings_differ() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let b = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![ConformanceWarning {
                code: "W001".into(),
                message: "something deprecated".into(),
                fields: serde_json::Map::new(),
            }],
        };
        assert_eq!(compare(&a, &b), DiffClass::WarningMismatch);
    }

    // --- compare: ErrorMismatch ---

    #[test]
    fn error_mismatch_when_codes_differ() {
        let a = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "same message".into(),
            context: serde_json::Map::new(),
        });
        let b = ConformanceResult::Err(ConformanceError {
            code: "E002".into(),
            message: "same message".into(),
            context: serde_json::Map::new(),
        });
        assert_eq!(compare(&a, &b), DiffClass::ErrorMismatch);
    }

    #[test]
    fn error_mismatch_when_context_differs() {
        let mut ctx = serde_json::Map::new();
        ctx.insert("key".into(), serde_json::Value::Bool(true));
        let a = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "same".into(),
            context: serde_json::Map::new(),
        });
        let b = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "same".into(),
            context: ctx,
        });
        assert_eq!(compare(&a, &b), DiffClass::ErrorMismatch);
    }

    // --- compare: UnsupportedOperation ---

    #[test]
    fn unsupported_operation_when_both_unsupported_with_different_reasons() {
        let a = ConformanceResult::Unsupported {
            reason: "reason A".into(),
        };
        let b = ConformanceResult::Unsupported {
            reason: "reason B".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::UnsupportedOperation);
    }

    #[test]
    fn unsupported_operation_when_only_one_side_unsupported() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Bool(true),
            warnings: vec![],
        };
        let b = ConformanceResult::Unsupported {
            reason: "n/a".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::UnsupportedOperation);
    }

    // --- compare: SetupFailure ---

    #[test]
    fn setup_failure_when_both_setup_failed_with_different_reasons() {
        let a = ConformanceResult::SetupFailed {
            reason: "timeout".into(),
        };
        let b = ConformanceResult::SetupFailed {
            reason: "missing env".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::SetupFailure);
    }

    #[test]
    fn setup_failure_when_only_one_side_setup_failed() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let b = ConformanceResult::SetupFailed {
            reason: "crash".into(),
        };
        assert_eq!(compare(&a, &b), DiffClass::SetupFailure);
    }

    // --- compare: VariantMismatch catch-all ---

    #[test]
    fn variant_mismatch_when_ok_vs_err() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let b = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "fail".into(),
            context: serde_json::Map::new(),
        });
        assert_eq!(compare(&a, &b), DiffClass::VariantMismatch);
    }

    // --- compare_per_step ---

    #[test]
    fn per_step_empty_slices_returns_empty_vec() {
        let result = compare_per_step(&[], &[]);
        assert_eq!(result, vec![]);
    }

    #[test]
    fn per_step_single_equal_pair_returns_match() {
        let a = ConformanceResult::Ok {
            value: ConformanceValue::Int(7),
            warnings: vec![],
        };
        let b = a.clone();
        assert_eq!(compare_per_step(&[a], &[b]), vec![DiffClass::Match]);
    }

    #[test]
    fn per_step_left_longer_reports_missing_field_for_extras() {
        let ok = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let left = vec![ok.clone(), ok.clone(), ok.clone()];
        let right = vec![ok.clone()];
        let result = compare_per_step(&left, &right);
        assert_eq!(
            result,
            vec![
                DiffClass::Match,
                DiffClass::MissingField,
                DiffClass::MissingField
            ]
        );
    }

    #[test]
    fn per_step_right_longer_reports_missing_field_for_extras() {
        let ok = ConformanceResult::Ok {
            value: ConformanceValue::Null,
            warnings: vec![],
        };
        let left = vec![ok.clone()];
        let right = vec![ok.clone(), ok.clone()];
        let result = compare_per_step(&left, &right);
        assert_eq!(result, vec![DiffClass::Match, DiffClass::MissingField]);
    }

    #[test]
    fn per_step_classifies_each_pair_independently() {
        let ok_a = ConformanceResult::Ok {
            value: ConformanceValue::Int(1),
            warnings: vec![],
        };
        let ok_b = ConformanceResult::Ok {
            value: ConformanceValue::Int(2),
            warnings: vec![],
        };
        let err = ConformanceResult::Err(ConformanceError {
            code: "E001".into(),
            message: "fail".into(),
            context: serde_json::Map::new(),
        });

        let left = vec![ok_a.clone(), ok_a.clone()];
        let right = vec![ok_a.clone(), ok_b.clone()];
        let result = compare_per_step(&left, &right);
        assert_eq!(result, vec![DiffClass::Match, DiffClass::ValueMismatch]);

        let left2 = vec![ok_a.clone()];
        let right2 = vec![err.clone()];
        let result2 = compare_per_step(&left2, &right2);
        assert_eq!(result2, vec![DiffClass::VariantMismatch]);
    }
}
