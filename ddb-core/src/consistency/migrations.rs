use crate::error::Result;
use crate::traits::DoogatStore;
use crate::types::{DoogatFix, Fix, FixReport, ParsedDoogat};

// ── Migration framework ──────────────────────────────────────────

/// A field-level migration that transforms doogats during format evolution.
pub struct Migration {
    pub version: u32,
    pub name: &'static str,
    pub apply: fn(&mut ParsedDoogat) -> Vec<Fix>,
}

/// Built-in migrations for known field renames and type normalizations.
pub(crate) fn built_in_migrations() -> Vec<Migration> {
    vec![
        Migration {
            version: 1,
            name: "zkn-id-to-id",
            apply: |p| {
                if let Some(value) = p.meta.extra.get("zkn-id").cloned() {
                    if p.meta.id.is_none() {
                        if let Some(s) = value.as_str() {
                            p.meta.id = Some(crate::types::DoogatId(s.to_string()));
                        }
                    }
                    return vec![Fix::FieldRenamed {
                        old: "zkn-id".into(),
                        new: "id".into(),
                    }];
                }
                vec![]
            },
        },
        Migration {
            version: 2,
            name: "tag-to-tags",
            apply: |p| {
                if let Some(value) = p.meta.extra.get("tag").cloned() {
                    if let Some(s) = value.as_str() {
                        if p.meta.tags.is_empty() {
                            p.meta.tags = vec![s.to_string()];
                        }
                    }
                    return vec![Fix::FieldRenamed {
                        old: "tag".into(),
                        new: "tags".into(),
                    }];
                }
                vec![]
            },
        },
        Migration {
            version: 3,
            name: "type-normalize",
            apply: |p| {
                let old_type = match p.meta.doogat_type.as_deref() {
                    Some(t) => t.to_string(),
                    None => return vec![],
                };
                let new_type = match old_type.as_str() {
                    "loop" => "project",
                    "wiki-article" | "doogat" => "note",
                    _ => return vec![],
                };
                p.meta.doogat_type = Some(new_type.to_string());
                vec![Fix::TypeNormalized {
                    old: old_type,
                    new: new_type.to_string(),
                }]
            },
        },
    ]
}

/// Read the current migration version from `.ddb/migration-version`.
pub(crate) fn read_migration_version(repo: &impl crate::traits::DoogatSource) -> u32 {
    repo.read_file(".ddb/migration-version")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Run pending migrations on all doogats.
///
/// Applies migrations with version > current, commits changes, and updates the version file.
pub fn migrate_all(repo: &impl DoogatStore, dry_run: bool) -> Result<FixReport> {
    let current_version = read_migration_version(repo);
    let migrations = built_in_migrations();
    let pending: Vec<&Migration> = migrations
        .iter()
        .filter(|m| m.version > current_version)
        .collect();

    if pending.is_empty() {
        return Ok(FixReport::default());
    }

    let max_version = pending.iter().map(|m| m.version).max().unwrap_or(0);
    let paths = repo.list_doogats()?;

    let mut report = FixReport {
        files_scanned: 0,
        files_fixed: 0,
        fixes: Vec::new(),
    };
    let mut writes: Vec<(String, String)> = Vec::new();

    for path in &paths {
        let content = match repo.read_file(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut parsed = match crate::parser::parse(&content, path) {
            Ok(p) => p,
            Err(_) => continue,
        };
        report.files_scanned += 1;

        let mut all_fixes = Vec::new();
        for migration in &pending {
            let fixes = (migration.apply)(&mut parsed);
            // Remove migrated fields from extras
            for fix in &fixes {
                if let Fix::FieldRenamed { old, .. } = fix {
                    parsed.meta.extra.remove(old);
                }
            }
            all_fixes.extend(fixes);
        }

        if all_fixes.is_empty() {
            continue;
        }

        if !dry_run {
            let new_content = crate::parser::serialize(&parsed);
            writes.push((path.clone(), new_content));
        }

        report.files_fixed += 1;
        report.fixes.push(DoogatFix {
            path: path.clone(),
            applied: all_fixes,
        });
    }

    if !dry_run {
        // Always include version file in the commit (even if no doogats changed)
        let version_content = max_version.to_string();
        writes.push((".ddb/migration-version".to_string(), version_content));

        let names: Vec<&str> = pending.iter().map(|m| m.name).collect();
        if report.files_fixed > 0 {
            let total_fixes: usize = report.fixes.iter().map(|f| f.applied.len()).sum();
            let msg = format!(
                "fix: migrate {} fields across {} doogats ({})",
                total_fixes,
                report.files_fixed,
                names.join(", ")
            );
            let write_refs: Vec<(&str, &str)> = writes
                .iter()
                .map(|(p, c)| (p.as_str(), c.as_str()))
                .collect();
            repo.commit_batch(&write_refs, &[], &msg)?;
        } else {
            // No doogats affected, but still advance the version
            repo.commit_file(
                ".ddb/migration-version",
                &max_version.to_string(),
                &format!(
                    "fix: advance migration version to {max_version} ({})",
                    names.join(", ")
                ),
            )?;
        }
    }

    Ok(report)
}
