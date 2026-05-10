use std::path::PathBuf;

use ddb_core::service::DoogatService;

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
    let body_text = body.unwrap_or_default();
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
    let parsed = svc.create_doogat_with_extra(
        &title,
        &tags_list,
        r#type.as_deref(),
        &body_text,
        extra,
    )?;
    outln!(
        "{}",
        parsed.meta.id.map(|z| z.0).unwrap_or_default()
    )?;
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

pub(crate) fn update(
    repo: &std::path::Path,
    args: UpdateArgs,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let tags_vec: Option<Vec<String>> =
        args.tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
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
    let filename = file.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        ddb_core::error::DoogatError::Validation("invalid filename".into())
    })?;
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
