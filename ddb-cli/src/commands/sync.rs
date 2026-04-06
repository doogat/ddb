use ddb_core::service::DoogatService;

use crate::{BundleAction, NodeAction};

pub(crate) fn sync(
    repo: &std::path::Path,
    remote: &str,
    branch: &str,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let report = svc.sync(remote, branch)?;
    outln!(
        "sync: {} | commits: {} | conflicts resolved: {}",
        report.direction,
        report.commits_transferred,
        report.conflicts_resolved
    )?;
    if report.collisions_reassigned > 0 {
        outln!(
            "  collisions reassigned: {}",
            report.collisions_reassigned
        )?;
    }
    Ok(())
}

pub(crate) fn register_node(repo: &std::path::Path, name: &str) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    let node = svc.register_node(name)?;
    outln!("registered node {} ({})", node.name, node.uuid)?;
    Ok(())
}

pub(crate) fn node(
    repo: &std::path::Path,
    action: NodeAction,
) -> ddb_core::error::Result<()> {
    match action {
        NodeAction::List => {
            let svc = DoogatService::open(repo)?;
            let nodes = svc.list_nodes()?;
            if nodes.is_empty() {
                outln!("no registered nodes")?;
            } else {
                for n in &nodes {
                    outln!(
                        "{} {} ({:?}) last_sync: {}",
                        n.uuid,
                        n.name,
                        n.status,
                        n.last_sync.as_deref().unwrap_or("never")
                    )?;
                }
            }
        }
        NodeAction::Retire { uuid } => {
            let svc = DoogatService::open(repo)?;
            svc.retire_node(&uuid)?;
            outln!("retired node {uuid}")?;
        }
    }
    Ok(())
}

pub(crate) fn bundle(
    repo: &std::path::Path,
    action: BundleAction,
) -> ddb_core::error::Result<()> {
    match action {
        BundleAction::Export {
            target,
            full,
            output,
        } => {
            let svc = DoogatService::open(repo)?;
            if full {
                let path = svc.export_full_bundle(&output)?;
                outln!("exported full bundle to {}", path.display())?;
            } else if let Some(target_uuid) = target {
                let path = svc.export_delta_bundle(&target_uuid, &output)?;
                outln!("exported delta bundle to {}", path.display())?;
            } else {
                return Err(ddb_core::error::DoogatError::Validation(
                    "specify --target <uuid> or --full".into(),
                ));
            }
        }
        BundleAction::Import { path } => {
            let svc = DoogatService::open(repo)?;
            let report = svc.import_bundle(&path)?;
            outln!(
                "imported: conflicts resolved: {}",
                report.conflicts_resolved
            )?;
        }
    }
    Ok(())
}
