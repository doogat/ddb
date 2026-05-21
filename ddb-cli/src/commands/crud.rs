use std::path::PathBuf;

use ddb_core::app_contract::CreateCommand;
use ddb_core::service::DoogatService;
use ddb_core::types::ConflictAction;

use crate::parse_set_pairs;

pub(crate) fn init(repo: &std::path::Path, path: Option<PathBuf>) -> ddb_core::error::Result<()> {
    let p = path.unwrap_or_else(|| repo.to_path_buf());
    DoogatService::init(&p)?;
    outln!("initialized ddb at {}", p.display())?;
    Ok(())
}

pub(crate) fn create(
    repo: &std::path::Path,
    title: String,
    tags: Option<String>,
    r#type: Option<String>,
    body: Option<String>,
    set: Vec<String>,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    // PRD 00136 / #16: defence-in-depth, mirroring `attach`/`detach`.
    // The service-layer `create_doogat_with_extra` already calls
    // `ensure_fresh` (PRD 00136 T2), so this CLI-level call is
    // redundant today; it exists so a future refactor that drops the
    // service-layer guard still leaves freshness intact at the CLI
    // boundary. `rebuild_if_stale` itself respects `skip_stale_check`,
    // so both layers honour the actor's opt-out. The call is cheap
    // (HEAD-unchanged short-circuit) and keeps the CLI's freshness
    // behaviour self-consistent across the create / update / delete /
    // attach / detach surfaces.
    svc.rebuild_if_stale()?;
    let tags_list: Vec<String> = tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let body_text = body;
    let mut extra = parse_set_pairs(&set)?;

    if r#type.as_deref() == Some("_typedef") {
        eprintln!("Warning: type definitions should be created with CREATE TABLE via 'ddb query'.");
        eprintln!("Manual typedefs are not CRDT-tracked and may be stripped by 'ddb fix'.");
        eprintln!("See: ddb help create-app");
        extra.insert(
            "origin".to_string(),
            ddb_core::types::Value::String("manual".into()),
        );
    }
    let cmd = CreateCommand {
        title: Some(title),
        tags: tags_list,
        doogat_type: r#type,
        body: body_text,
        fields: extra,
        on_conflict: ConflictAction::Error,
    };
    let output = svc.create(cmd)?;
    crate::warnings::write_warnings(&output.warnings, &mut std::io::stderr())
        .unwrap_or_else(|e| eprintln!("warning write failed: {e}"));
    outln!("{}", output.value.meta.id.map(|z| z.0).unwrap_or_default())?;
    Ok(())
}

pub(crate) fn read(repo: &std::path::Path, id: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let content = svc.read_doogat(id)?;
    out!("{content}")?;
    Ok(())
}

pub(crate) struct UpdateArgs {
    pub id: String,
    pub title: Option<String>,
    pub tags: Option<String>,
    pub r#type: Option<String>,
    pub body: Option<String>,
    pub set: Vec<String>,
    pub unset: Vec<String>,
}

pub(crate) fn update(repo: &std::path::Path, args: UpdateArgs) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let tags_vec: Option<Vec<String>> = args
        .tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
    let extra_map = parse_set_pairs(&args.set)?;
    let extra = ddb_core::service::ExtraFieldUpdates {
        set: &extra_map,
        unset: &args.unset,
    };
    svc.update_doogat(
        &args.id,
        args.title.as_deref(),
        tags_vec.as_deref(),
        args.r#type.as_deref(),
        args.body.as_deref(),
        &extra,
    )?;
    outln!("updated {}", args.id)?;
    Ok(())
}

pub(crate) fn delete(repo: &std::path::Path, id: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let broken = svc.delete_doogat(id, &format!("delete doogat {id}"))?;
    if !broken.is_empty() {
        eprintln!(
            "warning: {} doogat(s) have broken backlinks after deleting {id}:",
            broken.len()
        );
        for (src_id, src_path) in &broken {
            eprintln!("  - {src_id} ({src_path})");
        }
    }
    Ok(())
}

pub(crate) fn rename(
    repo: &std::path::Path,
    id: &str,
    new_path: &str,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let report = svc.rename_doogat(id, new_path)?;
    outln!("{} backlinks updated", report.updated.len())?;
    if !report.unresolvable.is_empty() {
        outln!("unresolvable:")?;
        for u in &report.unresolvable {
            outln!("  {u}")?;
        }
    }
    Ok(())
}

pub(crate) fn attach(
    repo: &std::path::Path,
    id: &str,
    file: &std::path::Path,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    svc.rebuild_if_stale()?;

    let bytes = std::fs::read(file)?;
    let filename = file
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| ddb_core::error::DoogatError::Validation("invalid filename".into()))?;
    let mime = ddb_core::types::AttachmentInfo::mime_from_filename(filename);
    let info = svc.attach_file(id, filename, &bytes, mime)?;
    outln!(
        "attached {} ({}, {} bytes)",
        info.name,
        info.mime,
        info.size
    )?;
    Ok(())
}

pub(crate) fn detach(
    repo: &std::path::Path,
    id: &str,
    filename: &str,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    svc.rebuild_if_stale()?;
    svc.detach_file(id, filename)?;
    outln!("detached {}", filename)?;
    Ok(())
}

pub(crate) fn attachments(repo: &std::path::Path, id: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let list = svc.list_attachments(id)?;
    if list.is_empty() {
        outln!("no attachments")?;
    } else {
        for a in &list {
            outln!("{}\t{}\t{} bytes", a.name, a.mime, a.size)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(dir: &std::path::Path) {
        DoogatService::init(dir).unwrap();
    }

    #[test]
    fn cli_create_stores_doogat_with_title() {
        let tmp = tempfile::TempDir::new().unwrap();
        init_repo(tmp.path());
        create(
            tmp.path(),
            "CLI Title".to_string(),
            None,
            None,
            None,
            vec![],
        )
        .unwrap();
        let svc = DoogatService::open(tmp.path()).unwrap();
        let results = svc.search("CLI Title").unwrap();
        assert!(!results.is_empty(), "doogat not found after CLI create");
        assert_eq!(results[0].title.as_str(), "CLI Title");
    }

    #[test]
    fn cli_create_stores_doogat_with_tags() {
        let tmp = tempfile::TempDir::new().unwrap();
        init_repo(tmp.path());
        create(
            tmp.path(),
            "Tagged CLI".to_string(),
            Some("foo,bar".to_string()),
            None,
            None,
            vec![],
        )
        .unwrap();
        let svc = DoogatService::open(tmp.path()).unwrap();
        let results = svc.search("Tagged CLI").unwrap();
        assert!(!results.is_empty());
        let parsed = svc.get_doogat_parsed(&results[0].id).unwrap();
        assert!(parsed.meta.tags.contains(&"foo".to_string()));
        assert!(parsed.meta.tags.contains(&"bar".to_string()));
    }

    #[test]
    fn cli_create_stores_doogat_with_set_field() {
        let tmp = tempfile::TempDir::new().unwrap();
        init_repo(tmp.path());
        create(
            tmp.path(),
            "Fields CLI".to_string(),
            None,
            None,
            None,
            vec!["priority=high".to_string()],
        )
        .unwrap();
        let svc = DoogatService::open(tmp.path()).unwrap();
        let results = svc.search("Fields CLI").unwrap();
        assert!(!results.is_empty());
        let parsed = svc.get_doogat_parsed(&results[0].id).unwrap();
        assert_eq!(
            parsed.meta.extra.get("priority"),
            Some(&ddb_core::types::Value::String("high".to_string()))
        );
    }
}
