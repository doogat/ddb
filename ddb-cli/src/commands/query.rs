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
    limit: usize,
    offset: usize,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let result = svc.search_paginated(query, limit, offset)?;
    if result.hits.is_empty() {
        outln!("no results")?;
    } else {
        let start = offset + 1;
        let end = offset + result.hits.len();
        outln!("Showing {start}-{end} of {} results", result.total_count)?;
        for r in &result.hits {
            outln!("[{}] {} ({})", r.id, r.title, r.path)?;
            outln!("  {}", r.snippet)?;
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
