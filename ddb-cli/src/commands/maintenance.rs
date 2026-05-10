use std::path::PathBuf;

use ddb_core::service::DoogatService;

use crate::{fmt_bytes, AutoAction, MaintenanceAction};

pub(crate) fn compact(
    repo: &std::path::Path,
    force: bool,
    dry_run: bool,
    no_backup: bool,
    backup_path: Option<PathBuf>,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    if dry_run {
        let info = svc.compact_dry_run()?;
        outln!("shared head: {:?}", info.shared_head)?;
        outln!("crdt temp files: {}", info.crdt_temp_files)?;
        if no_backup {
            outln!("backup: skipped")?;
        } else {
            let bp = backup_path.clone().unwrap_or(info.default_backup_path);
            outln!("backup would write: {}", bp.display())?;
        }
        outln!("(dry run — no changes made)")?;
    } else {
        let opts = ddb_core::types::CompactOptions {
            force,
            skip_backup: no_backup,
            backup_path,
        };
        let report = svc.compact(&opts)?;
        if let Some(ref bp) = report.backup_path {
            outln!("backup: {}", bp.display())?;
        }
        outln!(
            "files removed: {} | crdt compacted: {} | gc: {}",
            report.files_removed,
            report.crdt_docs_compacted,
            if report.gc_success { "ok" } else { "failed" }
        )?;
        outln!(
            "crdt temp: {} → {} ({} files → {})",
            fmt_bytes(report.crdt_temp_bytes_before),
            fmt_bytes(report.crdt_temp_bytes_after),
            report.crdt_temp_files_before,
            report.crdt_temp_files_after
        )?;
        outln!(
            "repo (.git): {} → {}",
            fmt_bytes(report.repo_bytes_before),
            fmt_bytes(report.repo_bytes_after)
        )?;
    }
    Ok(())
}

pub(crate) fn reindex(repo: &std::path::Path) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let report = svc.reindex()?;
    outln!("indexed {} doogats", report.indexed)?;
    if !report.warnings.is_empty() {
        outln!("{} warning(s)", report.warnings.len())?;
    }
    Ok(())
}

pub(crate) fn fix(
    repo: &std::path::Path,
    dry_run: bool,
    verbose: bool,
    migrate: bool,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    svc.rebuild_if_stale()?;

    if migrate {
        let mig_report = svc.migrate_all(dry_run)?;
        let mig_fixes: usize = mig_report.fixes.iter().map(|f| f.applied.len()).sum();
        if mig_fixes > 0 {
            if verbose {
                for zf in &mig_report.fixes {
                    outln!("  {}", zf.path)?;
                    for fix in &zf.applied {
                        outln!("    [{}] {fix}", fix.severity())?;
                    }
                }
            }
            if dry_run {
                outln!(
                    "would migrate {} fields in {} doogats",
                    mig_fixes,
                    mig_report.files_fixed
                )?;
            } else {
                outln!(
                    "migrated {} fields in {} doogats",
                    mig_fixes,
                    mig_report.files_fixed
                )?;
            }
        }

        let zone_report = svc.zone_migrate_all(dry_run)?;
        let zone_fixes: usize = zone_report.fixes.iter().map(|f| f.applied.len()).sum();
        if zone_fixes > 0 {
            if verbose {
                for zf in &zone_report.fixes {
                    outln!("  {}", zf.path)?;
                    for fix in &zf.applied {
                        outln!("    [{}] {fix}", fix.severity())?;
                    }
                }
            }
            if dry_run {
                outln!(
                    "would zone-migrate {} columns in {} doogats",
                    zone_fixes,
                    zone_report.files_fixed
                )?;
            } else {
                outln!(
                    "zone-migrated {} columns in {} doogats",
                    zone_fixes,
                    zone_report.files_fixed
                )?;
            }
        }
    }

    let report = svc.fix_all(dry_run)?;
    let total_fixes: usize = report.fixes.iter().map(|f| f.applied.len()).sum();

    if verbose {
        for zf in &report.fixes {
            outln!("  {}", zf.path)?;
            for fix in &zf.applied {
                outln!("    [{}] {fix}", fix.severity())?;
            }
        }
    }

    if dry_run {
        if total_fixes > 0 {
            outln!(
                "would fix {} issues in {} of {} doogats",
                total_fixes,
                report.files_fixed,
                report.files_scanned
            )?;
        } else {
            outln!("no issues found")?;
        }
    } else if total_fixes > 0 {
        outln!(
            "fixed {} issues in {} doogats",
            total_fixes,
            report.files_fixed
        )?;
    } else {
        outln!("no issues found")?;
    }
    Ok(())
}

pub(crate) fn status(repo: &std::path::Path) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let head = svc.head_oid()?;
    let stale = svc.is_index_stale()?;

    let node_uuid = std::fs::read_to_string(repo.join(".git/ddb-node"))
        .unwrap_or_else(|_| "not registered".into());

    let mut stale_nodes = Vec::new();
    let node_count = if let Ok(nodes) = svc.list_nodes() {
        for n in &nodes {
            if n.status == ddb_core::types::NodeStatus::Stale {
                stale_nodes.push(format!("{} ({})", n.name, n.uuid));
            }
        }
        nodes.len()
    } else {
        0
    };

    outln!("head: {head}")?;
    outln!("node: {}", node_uuid.trim())?;
    outln!("index stale: {stale}")?;
    outln!("registered nodes: {node_count}")?;
    if !stale_nodes.is_empty() {
        outln!("stale nodes: {}", stale_nodes.join(", "))?;
    }

    let resurrected = svc.resurrected_doogats().unwrap_or_default();
    if !resurrected.is_empty() {
        outln!("resurrected doogats: {}", resurrected.len())?;
        for (id, title) in &resurrected {
            outln!("  {id} {title}")?;
        }
    }

    let broken = svc.broken_backlinks().unwrap_or_default();
    if !broken.is_empty() {
        outln!("broken backlinks:")?;
        for (src_id, target_path) in &broken {
            outln!("  {src_id} -> {target_path}")?;
        }
    }
    Ok(())
}

pub(crate) fn maintenance(
    repo: &std::path::Path,
    action: MaintenanceAction,
) -> ddb_core::error::Result<()> {
    match action {
        MaintenanceAction::Run { task } => {
            let svc = DoogatService::open(repo)?;
            let tasks_slice: Vec<&str>;
            let tasks_opt = match &task {
                Some(t) => {
                    tasks_slice = vec![t.as_str()];
                    Some(tasks_slice.as_slice())
                }
                None => None,
            };
            let report = svc.run_maintenance(tasks_opt)?;
            outln!(
                "maintenance: {} | {}ms{}",
                if report.success { "ok" } else { "failed" },
                report.duration_ms,
                if report.fallback_used {
                    " (fallback: git gc)"
                } else {
                    ""
                }
            )?;
        }
        MaintenanceAction::Auto { action: auto } => {
            let svc = DoogatService::open(repo)?;
            match auto {
                AutoAction::Status => {
                    let config = svc.load_config()?;
                    outln!(
                        "{}",
                        if config.maintenance.auto_enabled {
                            "on"
                        } else {
                            "off"
                        }
                    )?;
                }
                AutoAction::On | AutoAction::Off => {
                    let enabled = matches!(auto, AutoAction::On);
                    svc.set_auto_maintenance(enabled)?;
                    outln!(
                        "auto-maintenance {}",
                        if enabled { "enabled" } else { "disabled" }
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn serve(
    repo: &std::path::Path,
    port: u16,
    pg_port: u16,
    bind: &str,
    playground: bool,
) -> ddb_core::error::Result<()> {
    let repo_path = std::fs::canonicalize(repo)?;
    let rt = tokio::runtime::Runtime::new().map_err(ddb_core::error::DoogatError::Io)?;
    rt.block_on(async {
        ddb_server::run(repo_path, Some(port), Some(pg_port), Some(bind), playground)
            .await
            .map_err(ddb_core::error::DoogatError::Io)
    })?;
    Ok(())
}
