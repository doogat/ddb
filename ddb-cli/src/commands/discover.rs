use ddb_core::parser;
use ddb_core::service::DoogatService;

use crate::{DiscoverAction, SequenceAction, TypeAction};

pub(crate) fn discover(
    repo: &std::path::Path,
    action: DiscoverAction,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    svc.rebuild_if_stale()?;

    match action {
        DiscoverAction::Mentions { id, all } => {
            if all {
                let all_ids = svc.all_doogat_ids()?;
                let mut total = 0usize;
                for zid in &all_ids {
                    let mentions = svc.unlinked_mentions(zid)?;
                    if !mentions.is_empty() {
                        total += mentions.len();
                        outln!("{zid}\t{} mention(s)", mentions.len())?;
                    }
                }
                if total == 0 {
                    outln!("no unlinked mentions found")?;
                }
            } else if let Some(id) = id {
                let mentions = svc.unlinked_mentions(&id)?;
                if mentions.is_empty() {
                    outln!("no unlinked mentions")?;
                } else {
                    for m in &mentions {
                        outln!("{}\t{}\t{}", m.source_id, m.source_title, m.snippet)?;
                    }
                }
            } else {
                return Err(ddb_core::error::DoogatError::Validation(
                    "specify a doogat ID or --all".into(),
                ));
            }
        }
        DiscoverAction::Similar { id, limit } => {
            let suggestions = svc.suggest_links(&id, limit)?;
            if suggestions.is_empty() {
                outln!("no suggestions")?;
            } else {
                for s in &suggestions {
                    let tags = if s.shared_tags.is_empty() {
                        String::new()
                    } else {
                        s.shared_tags.join(", ")
                    };
                    outln!("{}\t{}\t{:.2}\t{}", s.id, s.title, s.score, tags)?;
                }
            }
        }
        DiscoverAction::Stale { type_filter } => {
            let stale = svc.stale_doogats(type_filter.as_deref())?;
            if stale.is_empty() {
                outln!("no stale doogats")?;
            } else {
                for s in &stale {
                    outln!(
                        "{}\t{}\t{}\t{}\t{}d stale",
                        s.id,
                        s.title,
                        s.doogat_type,
                        s.last_updated,
                        s.days_stale
                    )?;
                }
            }
        }
        DiscoverAction::Orphans { type_filter } => {
            let orphans = svc.orphan_doogats(type_filter.as_deref())?;
            if orphans.is_empty() {
                outln!("no orphan doogats")?;
            } else {
                for o in &orphans {
                    outln!(
                        "{}\t{}\t{}\t{} outgoing",
                        o.id,
                        o.title,
                        o.doogat_type,
                        o.outgoing_links
                    )?;
                }
            }
        }
        DiscoverAction::Recent { days, type_filter } => {
            let recent = svc.recent_doogats(days, type_filter.as_deref())?;
            if recent.is_empty() {
                outln!("no recent doogats")?;
            } else {
                for r in &recent {
                    outln!(
                        "{}\t{}\t{}\t{}",
                        r.id,
                        r.title,
                        r.doogat_type,
                        r.last_modified
                    )?;
                }
            }
        }
        DiscoverAction::LinkDensity { type_filter } => {
            let entries = svc.link_density(type_filter.as_deref())?;
            if entries.is_empty() {
                outln!("no doogats found")?;
            } else {
                for e in &entries {
                    outln!(
                        "{}\t{}\t{}\tin:{}\tout:{}\tdensity:{}",
                        e.id,
                        e.title,
                        e.doogat_type,
                        e.inbound_links,
                        e.outbound_links,
                        e.density_score
                    )?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn sequence(
    repo: &std::path::Path,
    action: SequenceAction,
) -> ddb_core::error::Result<()> {
    let svc = DoogatService::open(repo)?;
    svc.rebuild_if_stale()?;

    match action {
        SequenceAction::Tree { id } => {
            let tree = svc.sequence_tree(&id, 100)?;
            for (node, depth) in &tree {
                let indent = "  ".repeat(*depth);
                outln!("{}{} {}", indent, node.id, node.title)?;
            }
        }
        SequenceAction::Breadcrumb { id } => {
            let bc = svc.sequence_breadcrumb(&id)?;
            for n in &bc {
                outln!("{}\t{}", n.id, n.title)?;
            }
        }
        SequenceAction::Broken => {
            let broken = svc.broken_sequences()?;
            if broken.is_empty() {
                outln!("no broken sequences")?;
            } else {
                for b in &broken {
                    outln!("{} -> {} (not found)", b.doogat_id, b.broken_parent_id)?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn type_cmd(repo: &std::path::Path, action: TypeAction) -> ddb_core::error::Result<()> {
    match action {
        TypeAction::Suggest { name } => {
            let svc = DoogatService::open(repo)?;
            let schema = svc.infer_schema(&name)?;
            if schema.columns.is_empty() {
                eprintln!("no data found for type \"{}\"", name);
                std::process::exit(1);
            }

            let id = parser::generate_id();
            let doogat = ddb_core::sql_engine::build_typedef_doogat(&id, &schema);
            out!("{}", parser::serialize(&doogat))?;
        }
        TypeAction::Install { name } => {
            let svc = DoogatService::open(repo)?;
            let id = svc.install_bundled_type(&name)?;
            outln!("installed type \"{name}\" as {id}")?;
        }
    }
    Ok(())
}
