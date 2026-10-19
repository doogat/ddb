use rusqlite::params;

use crate::types::{LinkDensityEntry, Suggestion};

/// Parse a date string (ISO 8601 or YYYY-MM-DD) into a NaiveDate.
pub(super) fn parse_date_to_naive(s: &str) -> Option<chrono::NaiveDate> {
    // Try ISO 8601 datetime first (e.g. 2026-03-16T20:51:04+00:00)
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.date_naive());
    }
    // Try date-only (e.g. 2026-03-16)
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(d);
    }
    // Try ISO 8601 with non-standard offset format (e.g. 2026-03-16T20:51:04+0000)
    if let Ok(dt) = chrono::DateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%z") {
        return Some(dt.date_naive());
    }
    None
}

/// Resolve the most relevant date for a doogat using the priority chain:
/// git revision date, then frontmatter date, then indexer updated_at.
pub(super) fn resolve_last_date(
    repo: &(impl crate::traits::DoogatSource + crate::traits::GitHistory),
    path: &str,
    fm_date: Option<&str>,
    updated_at: Option<&str>,
) -> Option<(String, crate::types::DateSource)> {
    use crate::types::DateSource;

    if let Ok(Some(git_date)) = repo.revision_date(path) {
        Some((git_date, DateSource::GitRevision))
    } else if let Some(d) = fm_date {
        Some((d.to_string(), DateSource::FrontmatterDate))
    } else {
        updated_at.map(|u| (u.to_string(), DateSource::IndexerUpdatedAt))
    }
}

/// Parse a date, compute days since, and build a `StaleDoogat` if past threshold.
pub(super) fn compute_staleness(
    id: &str,
    title: String,
    doogat_type: String,
    last_date: &str,
    source: crate::types::DateSource,
    threshold: u32,
    today: chrono::NaiveDate,
) -> Option<crate::types::StaleDoogat> {
    let naive = parse_date_to_naive(last_date)?;
    let days_since = (today - naive).num_days();
    if days_since < 0 {
        return None;
    }
    let days_since = days_since as u32;
    if days_since <= threshold {
        return None;
    }
    Some(crate::types::StaleDoogat {
        id: id.to_string(),
        title,
        doogat_type,
        last_updated: last_date.to_string(),
        date_source: source,
        days_stale: days_since - threshold,
        threshold_days: threshold,
    })
}

pub(super) fn count_links(
    id: &str,
    title: &str,
    doogat_type: &str,
    path: &str,
    out_stmt: &mut rusqlite::Statement<'_>,
    in_stmt: &mut rusqlite::Statement<'_>,
) -> LinkDensityEntry {
    let outbound: usize = out_stmt
        .query_row(params![id], |row| row.get::<_, i64>(0))
        .unwrap_or(0) as usize;

    let inbound: usize = in_stmt
        .query_row(params![id, path], |row| row.get::<_, i64>(0))
        .unwrap_or(0) as usize;

    LinkDensityEntry {
        id: id.to_string(),
        title: title.to_string(),
        doogat_type: doogat_type.to_string(),
        inbound_links: inbound,
        outbound_links: outbound,
        density_score: inbound + outbound,
    }
}

pub(super) fn normalize_suggestion_scores(mut results: Vec<Suggestion>) -> Vec<Suggestion> {
    let max = results.iter().map(|s| s.score).fold(0.0_f64, f64::max);
    if max > 0.0 {
        for s in &mut results {
            s.score /= max;
        }
    }
    results
}
