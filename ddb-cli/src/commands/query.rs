use ddb_core::app_contract::{PageRequest, SearchCommand};
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;

use crate::CREATE_APP_GUIDE;

pub(crate) fn help(topic: Option<String>) -> ddb_core::error::Result<()> {
    match topic.as_deref() {
        Some("create-app") => outln!("{CREATE_APP_GUIDE}")?,
        Some(other) => {
            return Err(ddb_core::error::DoogatError::Validation(format!(
                "unknown guide: {other}\n\nAvailable guides:\n  create-app    Data modeling, zones, title resolution, and API access"
            )));
        }
        None => {
            outln!("Available guides:")?;
            outln!("  create-app    Data modeling, zones, title resolution, and API access")?;
            outln!("")?;
            outln!("Usage: ddb help <topic>")?;
        }
    }
    Ok(())
}

pub(crate) fn query(repo: &std::path::Path, sql: &str) -> ddb_core::error::Result<()> {
    let mut svc = DoogatService::open(repo)?;
    svc.rebuild_if_stale()?;
    for result in svc.execute_batch(sql)? {
        match result {
            SqlResult::Rows { rows, .. } => {
                for row in rows {
                    outln!("{}", row.join(" | "))?;
                }
            }
            SqlResult::Affected(n) => outln!("{n} row(s) affected")?,
            SqlResult::Ok(msg) => outln!("{msg}")?,
        }
    }
    Ok(())
}

pub(crate) fn search(
    repo: &std::path::Path,
    query: &str,
    limit: Option<usize>,
    offset: Option<usize>,
    doogat_type: Option<String>,
    tag: Option<String>,
) -> ddb_core::error::Result<()> {
    let to_i64 = |n: usize| i64::try_from(n).unwrap_or(i64::MAX);
    let page = PageRequest::decode(limit.map(to_i64), offset.map(to_i64))?;
    let svc = DoogatService::open(repo)?;
    let output = svc.search_command(SearchCommand {
        query: query.to_string(),
        page,
        types: doogat_type.map(|t| vec![t]),
        tag,
    })?;
    crate::warnings::write_warnings(&output.warnings, &mut std::io::stderr())
        .unwrap_or_else(|e| eprintln!("warning write failed: {e}"));
    let result = output.value;
    if result.items.is_empty() {
        outln!("no results")?;
        return Ok(());
    }
    let start = page.offset + 1;
    let end = page.offset + result.items.len();
    outln!("Showing {start}-{end} of {} results", result.total)?;
    for r in &result.items {
        outln!("[{}] {} ({})", r.id, r.title, r.path)?;
        outln!("  {}", r.snippet)?;
        outln!(
            "  type: {}  rank: {}  tags: {}",
            r.doogat_type.as_deref().unwrap_or("-"),
            r.rank,
            r.tags.join(", ")
        )?;
        outln!(
            "  created: {}  updated: {}",
            r.created_at.as_deref().unwrap_or("-"),
            r.updated_at
        )?;
        if let Some(fields) = &r.fields {
            for (k, v) in fields {
                outln!("  {k}: {v}")?;
            }
        }
    }
    Ok(())
}

pub(crate) fn get(repo: &std::path::Path, id: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let content = svc.read_doogat(id)?;
    out!("{content}")?;
    Ok(())
}

pub(crate) fn scan(
    repo: &std::path::Path,
    r#type: Option<String>,
    tag: Option<String>,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let ids = if let Some(t) = r#type {
        svc.nosql_scan_type(&t)?
    } else if let Some(t) = tag {
        svc.nosql_scan_tag(&t)?
    } else {
        return Err(ddb_core::error::DoogatError::Validation(
            "specify --type or --tag".into(),
        ));
    };
    for id in &ids {
        outln!("{id}")?;
    }
    Ok(())
}

pub(crate) fn backlinks(repo: &std::path::Path, id: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let ids = svc.backlink_ids(id)?;
    for bl in &ids {
        outln!("{bl}")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use ddb_core::error::DoogatError;

    /// Init a repo with two doogats sharing `cliterm`, differing in type/tags.
    fn seeded_repo() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().unwrap();
        let svc = DoogatService::init(tmp.path()).unwrap();
        svc.create_doogat(
            "Soup",
            &["dinner".to_string()],
            Some("recipe"),
            "cliterm soup",
        )
        .unwrap();
        svc.create_doogat(
            "Cake",
            &["dinner".to_string(), "sweet".to_string()],
            Some("dessert"),
            "cliterm cake",
        )
        .unwrap();
        tmp
    }

    /// Parse `ddb search ...` argv and return the Search variant's fields
    /// as (query, limit, offset, doogat_type, tag).
    fn parse_search(
        argv: &[&str],
    ) -> (
        String,
        Option<usize>,
        Option<usize>,
        Option<String>,
        Option<String>,
    ) {
        let cli = crate::Cli::try_parse_from(argv).expect("search argv must parse");
        match cli.command {
            crate::Command::Search {
                query,
                limit,
                offset,
                doogat_type,
                tag,
            } => (query, limit, offset, doogat_type, tag),
            _ => panic!("expected the Search command"),
        }
    }

    fn assert_validation(result: ddb_core::error::Result<()>, what: &str) {
        match result {
            Err(DoogatError::Validation(_)) => {}
            other => panic!("{what}: expected DoogatError::Validation, got {other:?}"),
        }
    }

    #[test]
    fn omitted_limit_and_offset_reach_search_as_none_not_clap_defaults() {
        // None lets PageRequest::decode apply the shared 50/0 defaults;
        // the old clap default_value of 20/0 must be gone.
        let (query, limit, offset, doogat_type, tag) = parse_search(&["ddb", "search", "foo"]);
        assert_eq!(query, "foo");
        assert_eq!(limit, None, "an omitted --limit must not get a clap default");
        assert_eq!(offset, None, "an omitted --offset must not get a clap default");
        assert_eq!(doogat_type, None);
        assert_eq!(tag, None);
    }

    #[test]
    fn type_and_tag_flags_parse_into_search_filters() {
        let (query, limit, offset, doogat_type, tag) = parse_search(&[
            "ddb", "search", "foo", "--type", "recipe", "--tag", "sweet", "--limit", "5",
            "--offset", "2",
        ]);
        assert_eq!(query, "foo");
        assert_eq!(limit, Some(5));
        assert_eq!(offset, Some(2));
        assert_eq!(doogat_type.as_deref(), Some("recipe"));
        assert_eq!(tag.as_deref(), Some("sweet"));
    }

    #[test]
    fn cli_search_with_type_and_tag_filters_succeeds() {
        let tmp = seeded_repo();

        search(
            tmp.path(),
            "cliterm",
            None,
            None,
            Some("recipe".to_string()),
            None,
        )
        .expect("type-filtered search must succeed");
        search(
            tmp.path(),
            "cliterm",
            None,
            None,
            None,
            Some("sweet".to_string()),
        )
        .expect("tag-filtered search must succeed");
        search(
            tmp.path(),
            "cliterm",
            None,
            None,
            Some("dessert".to_string()),
            Some("dinner".to_string()),
        )
        .expect("type+tag-filtered search must succeed");
    }

    #[test]
    fn cli_search_with_omitted_limit_and_offset_succeeds() {
        let tmp = seeded_repo();

        search(tmp.path(), "cliterm", None, None, None, None)
            .expect("omitted limit/offset must decode to the default page");
    }

    #[test]
    fn cli_search_rejects_zero_limit_through_page_request() {
        let tmp = seeded_repo();

        assert_validation(
            search(tmp.path(), "cliterm", Some(0), None, None, None),
            "--limit 0",
        );
    }

    #[test]
    fn cli_search_rejects_limit_over_max_through_page_request() {
        let tmp = seeded_repo();

        assert_validation(
            search(tmp.path(), "cliterm", Some(10_001), None, None, None),
            "--limit 10001",
        );
    }
}
