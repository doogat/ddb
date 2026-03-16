use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use zdb_core::compaction;
use zdb_core::git_ops::{self, GitRepo};
use zdb_core::indexer::Index;
use zdb_core::parser;
use zdb_core::sql_engine::{SqlEngine, SqlResult};
use zdb_core::sync_manager::{self, SyncManager};

mod updater;

macro_rules! out {
    ($($arg:tt)*) => {
        write_stdout(format_args!($($arg)*))
    };
}

macro_rules! outln {
    ($($arg:tt)*) => {
        writeln_stdout(format_args!($($arg)*))
    };
}

fn write_stdout(args: std::fmt::Arguments<'_>) -> zdb_core::error::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.flush()?;
    Ok(())
}

fn writeln_stdout(args: std::fmt::Arguments<'_>) -> zdb_core::error::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn is_broken_pipe(err: &zdb_core::error::ZettelError) -> bool {
    matches!(err, zdb_core::error::ZettelError::Io(io_err) if io_err.kind() == io::ErrorKind::BrokenPipe)
}

#[derive(Parser)]
#[command(name = "zdb", version, about = "Decentralized Zettelkasten")]
struct Cli {
    /// Repository path (default: current directory)
    #[arg(short, long, default_value = ".")]
    repo: PathBuf,

    /// Directory for NDJSON log files (default: stderr with env filter)
    #[arg(long, global = true, env = "ZDB_LOG_DIR")]
    log_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new zettelkasten repository
    Init {
        /// Path to create the repository
        path: Option<PathBuf>,
    },
    /// Create a new zettel
    Create {
        #[arg(long)]
        title: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        body: Option<String>,
    },
    /// Read a zettel by ID
    Read {
        /// Zettel ID
        id: String,
    },
    /// Update an existing zettel
    Update {
        /// Zettel ID
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        body: Option<String>,
    },
    /// Delete a zettel by ID
    Delete {
        /// Zettel ID
        id: String,
    },
    /// Sync with remote
    Sync {
        /// Remote name
        #[arg(default_value = "origin")]
        remote: String,
        /// Branch name
        #[arg(default_value = "master")]
        branch: String,
    },
    /// Execute SQL (DDL/DML routed through SQL engine; SELECT queries index)
    Query {
        /// SQL statement
        sql: String,
    },
    /// Full-text search
    Search {
        /// Search query
        query: String,
        /// Maximum results to return
        #[arg(long, default_value = "20")]
        limit: usize,
        /// Number of results to skip
        #[arg(long, default_value = "0")]
        offset: usize,
    },
    /// Register this device as a sync node
    RegisterNode {
        /// Device name
        name: String,
    },
    /// Show repository status
    Status,
    /// Compact CRDT history and run git gc
    Compact {
        /// Force compaction even if under threshold
        #[arg(long)]
        force: bool,
        /// Show what would be done without doing it
        #[arg(long)]
        dry_run: bool,
        /// Skip pre-compaction backup bundle
        #[arg(long)]
        no_backup: bool,
        /// Custom path for backup bundle
        #[arg(long)]
        backup_path: Option<PathBuf>,
    },
    /// Rebuild the search index
    Reindex,
    /// Auto-fix consistency issues across zettels
    Fix {
        /// Report fixes without modifying files
        #[arg(long)]
        dry_run: bool,
        /// Show detailed fix list per zettel
        #[arg(short, long)]
        verbose: bool,
        /// Run pending field migrations before fixing
        #[arg(long)]
        migrate: bool,
    },
    /// Rename (move) a zettel and rewrite backlinks
    Rename {
        /// Zettel ID
        id: String,
        /// New file path (relative to repo root)
        new_path: String,
    },
    /// Type definition management
    Type {
        #[command(subcommand)]
        action: TypeAction,
    },
    /// Node management
    Node {
        #[command(subcommand)]
        action: NodeAction,
    },
    /// [experimental] Export/import bundles for air-gapped sync
    Bundle {
        #[command(subcommand)]
        action: BundleAction,
    },
    /// [experimental] Start GraphQL API server
    Serve {
        #[arg(long, default_value = "2891")]
        port: u16,
        #[arg(long, default_value = "2892")]
        pg_port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        bind: String,
        #[arg(long)]
        playground: bool,
    },
    /// [experimental] Attach a file to a zettel
    Attach {
        /// Zettel ID
        id: String,
        /// Path to the file to attach
        file: PathBuf,
    },
    /// [experimental] Detach a file from a zettel
    Detach {
        /// Zettel ID
        id: String,
        /// Filename to detach
        filename: String,
    },
    /// [experimental] List attachments on a zettel
    Attachments {
        /// Zettel ID
        id: String,
    },
    /// [experimental] Get zettel by ID via NoSQL index (O(1) lookup)
    Get {
        /// Zettel ID
        id: String,
    },
    /// [experimental] Prefix scan by type or tag via NoSQL index
    Scan {
        /// Filter by zettel type
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// [experimental] List backlinks via NoSQL index
    Backlinks {
        /// Zettel ID
        id: String,
    },
    /// Run git maintenance tasks
    Maintenance {
        #[command(subcommand)]
        action: MaintenanceAction,
    },
    /// [experimental] Update zdb to the latest release
    UpdateBin,
    /// Background update check (internal)
    #[command(name = "__update-check", hide = true)]
    UpdateCheck,
}

#[derive(Subcommand)]
enum NodeAction {
    /// List all registered nodes
    List,
    /// Retire a node by UUID
    Retire {
        /// Node UUID
        uuid: String,
    },
}

#[derive(Subcommand)]
enum TypeAction {
    /// Suggest a _typedef zettel from inferred schema
    Suggest {
        /// Type name to infer
        name: String,
    },
    /// Install a bundled type definition
    Install {
        /// Bundled type name (project, contact)
        name: String,
    },
}

#[derive(Subcommand)]
enum MaintenanceAction {
    /// Run maintenance tasks
    Run {
        /// Specific task (e.g. commit-graph, gc, incremental-repack)
        #[arg(long)]
        task: Option<String>,
    },
    /// Toggle or query auto-maintenance
    Auto {
        #[command(subcommand)]
        action: AutoAction,
    },
}

#[derive(Subcommand)]
enum AutoAction {
    /// Enable auto-maintenance after sync and compact
    On,
    /// Disable auto-maintenance
    Off,
    /// Show current auto-maintenance setting
    Status,
}

#[derive(Subcommand)]
enum BundleAction {
    /// Export a bundle for a target node
    Export {
        /// Target node UUID (or --full for bootstrap bundle)
        #[arg(long)]
        target: Option<String>,
        /// Export all refs (for bootstrapping new nodes)
        #[arg(long)]
        full: bool,
        /// Output file path
        #[arg(long, short)]
        output: PathBuf,
    },
    /// Import a bundle
    Import {
        /// Path to bundle tar file
        path: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    init_logging(cli.log_dir.as_deref());

    // Handle update commands before anything else
    match &cli.command {
        Command::UpdateCheck => {
            updater::check_and_update();
            return;
        }
        Command::UpdateBin => {
            if let Err(e) = updater::run_update() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
            return;
        }
        _ => {
            updater::notify_if_updated();
            updater::spawn_background_check();
        }
    }

    if let Err(e) = run(cli) {
        if is_broken_pipe(&e) {
            return;
        }
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn init_logging(log_dir: Option<&std::path::Path>) {
    use tracing_subscriber::fmt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));

    match log_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).ok();
            let date = chrono::Local::now().format("%Y-%m-%d");
            let path = dir.join(format!("zdb-{date}.ndjson"));
            match std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                Ok(file) => {
                    fmt()
                        .json()
                        .with_writer(file)
                        .with_env_filter(filter)
                        .with_target(true)
                        .init();
                }
                Err(e) => {
                    fmt()
                        .with_writer(std::io::stderr)
                        .with_env_filter(filter)
                        .init();
                    eprintln!(
                        "warning: failed to open log file {}: {e}, falling back to stderr",
                        path.display()
                    );
                }
            }
        }
        None => {
            fmt()
                .with_writer(std::io::stderr)
                .with_env_filter(filter)
                .init();
        }
    }
}

/// Generate a unique ID, spin-waiting if a zettel with that ID already exists on disk.
fn unique_id(repo_path: &std::path::Path) -> zdb_core::types::ZettelId {
    let zk = repo_path.join("zettelkasten");
    parser::generate_unique_id(|candidate| {
        let filename = format!("{candidate}.md");
        // Check root zettelkasten/ and one level of subdirectories
        if zk.join(&filename).exists() {
            return true;
        }
        if let Ok(entries) = std::fs::read_dir(&zk) {
            for entry in entries.flatten() {
                if entry.path().is_dir() && entry.path().join(&filename).exists() {
                    return true;
                }
            }
        }
        false
    })
}

/// Dual-write a zettel to the redb NoSQL index (best-effort).
fn redb_index_zettel(repo_path: &std::path::Path, zettel: &zdb_core::types::ParsedZettel) {
    let redb_path = repo_path.join(".zdb/nosql.redb");
    if let Ok(ri) = zdb_core::nosql::RedbIndex::open(&redb_path) {
        let _ = ri.index_zettel(zettel);
    }
}

/// Dual-remove a zettel from the redb NoSQL index (best-effort).
fn redb_remove_zettel(repo_path: &std::path::Path, id: &str) {
    let redb_path = repo_path.join(".zdb/nosql.redb");
    if let Ok(ri) = zdb_core::nosql::RedbIndex::open(&redb_path) {
        let _ = ri.remove_zettel(id);
    }
}

fn open_index(repo: &std::path::Path) -> zdb_core::error::Result<Index> {
    let db_path = repo.join(".zdb/index.db");
    let parent = db_path.parent().ok_or_else(|| {
        zdb_core::error::ZettelError::InvalidPath("cannot determine .zdb parent dir".into())
    })?;
    std::fs::create_dir_all(parent)?;
    Index::open(&db_path)
}

fn fmt_bytes(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else if b >= KB {
        format!("{:.1} KB", b as f64 / KB as f64)
    } else {
        format!("{b} B")
    }
}

fn run(cli: Cli) -> zdb_core::error::Result<()> {
    match cli.command {
        Command::Init { path } => {
            let p = path.unwrap_or_else(|| cli.repo.clone());
            GitRepo::init(&p)?;
            outln!("initialized zettelkasten at {}", p.display())?;
        }

        Command::Create {
            title,
            tags,
            r#type,
            body,
        } => {
            let repo = GitRepo::open(&cli.repo)?;
            let id = unique_id(&cli.repo);
            let tags_list: Vec<String> = tags
                .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();
            let body_text = body.unwrap_or_default();

            let id_str = id.to_string();
            let index = open_index(&cli.repo)?;
            let folder = r#type
                .as_deref()
                .map(|t| index.type_uses_folder(t, &repo))
                .unwrap_or(false);
            let path = zdb_core::git_ops::zettel_path(&id_str, r#type.as_deref(), folder);
            let commit_msg = format!("create zettel {id_str}");

            let meta = zdb_core::types::ZettelMeta {
                id: Some(id),
                title: Some(title),
                date: Some(chrono::Local::now().format("%Y-%m-%d").to_string()),
                zettel_type: r#type,
                tags: tags_list,
                extra: Default::default(),
            };

            let parsed = zdb_core::types::ParsedZettel {
                meta,
                body: body_text,
                sections: vec![],
                reference_section: String::new(),
                inline_fields: vec![],
                links: vec![],
                body_tags: vec![],
                checkboxes: vec![],
                path: path.clone(),
            };

            let content = parser::serialize(&parsed);
            repo.commit_file(&path, &content, &commit_msg)?;

            // Index (index already opened above for folder lookup)
            index.index_zettel(&parsed)?;
            redb_index_zettel(&cli.repo, &parsed);

            outln!("{id_str}")?;
        }

        Command::Read { id } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;
            let path = index.resolve_path(&id)?;
            let content = repo.read_file(&path)?;
            out!("{content}")?;
        }

        Command::Update {
            id,
            title,
            tags,
            r#type,
            body,
        } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;
            let path = index.resolve_path(&id)?;
            let content = repo.read_file(&path)?;
            let mut parsed = parser::parse(&content, &path)?;

            if let Some(t) = title {
                parsed.meta.title = Some(t);
            }
            if let Some(t) = tags {
                parsed.meta.tags = t.split(',').map(|s| s.trim().to_string()).collect();
            }
            if let Some(t) = r#type {
                parsed.meta.zettel_type = Some(t);
            }
            if let Some(b) = body {
                parsed.body = b;
            }

            let new_content = parser::serialize(&parsed);
            repo.commit_file(&path, &new_content, &format!("update zettel {id}"))?;
            index.index_zettel(&parsed)?;
            redb_index_zettel(&cli.repo, &parsed);

            outln!("updated {id}")?;
        }

        Command::Delete { id } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;
            let path = index.resolve_path(&id)?;
            let broken = index.backlinking_zettel_paths(&id)?;
            repo.delete_file(&path, &format!("delete zettel {id}"))?;
            index.remove_zettel(&id)?;
            redb_remove_zettel(&cli.repo, &id);
            if !broken.is_empty() {
                eprintln!(
                    "warning: {} zettel(s) have broken backlinks after deleting {id}:",
                    broken.len()
                );
                for (src_id, src_path) in &broken {
                    eprintln!("  - {src_id} ({src_path})");
                }
            }
        }

        Command::Sync { remote, branch } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            let mut mgr = SyncManager::open(&repo)?;

            let report = mgr.sync(&remote, &branch, &index)?;
            outln!(
                "sync: {} | commits: {} | conflicts resolved: {}",
                report.direction,
                report.commits_transferred,
                report.conflicts_resolved
            )?;
        }

        Command::Query { sql } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;

            let mut engine = SqlEngine::new(&index, &repo);
            for result in engine.execute_batch(&sql)? {
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
        }

        Command::Search {
            query,
            limit,
            offset,
        } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;

            let result = index.search_paginated(&query, limit, offset)?;
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
        }

        Command::RegisterNode { name } => {
            let repo = GitRepo::open(&cli.repo)?;
            let node = sync_manager::register_node(&repo, &name)?;
            outln!("registered node {} ({})", node.name, node.uuid)?;
        }

        Command::Status => {
            let repo = GitRepo::open(&cli.repo)?;
            let head = repo.head_oid()?;

            let db_path = cli.repo.join(".zdb/index.db");
            let stale = if db_path.exists() {
                let index = Index::open(&db_path)?;
                index.is_stale(&repo)?
            } else {
                true
            };

            let node_uuid = std::fs::read_to_string(cli.repo.join(".git/zdb-node"))
                .unwrap_or_else(|_| "not registered".into());

            let mut stale_nodes = Vec::new();
            let node_count = if let Ok(mgr) = SyncManager::open(&repo) {
                let nodes = mgr.list_nodes().unwrap_or_default();
                for n in &nodes {
                    if n.status == zdb_core::types::NodeStatus::Stale {
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

            // Check for resurrected zettels and broken backlinks
            if db_path.exists() {
                let index = Index::open(&db_path)?;
                let resurrected = index.resurrected_zettels().unwrap_or_default();
                if !resurrected.is_empty() {
                    outln!("resurrected zettels: {}", resurrected.len())?;
                    for (id, title) in &resurrected {
                        outln!("  {id} {title}")?;
                    }
                }

                let broken = index.broken_backlinks().unwrap_or_default();
                if !broken.is_empty() {
                    outln!("broken backlinks:")?;
                    for (src_id, target_path) in &broken {
                        outln!("  {src_id} -> {target_path}")?;
                    }
                }
            }
        }

        Command::Node { action } => match action {
            NodeAction::List => {
                let repo = GitRepo::open(&cli.repo)?;
                let mgr = SyncManager::open(&repo)?;
                let nodes = mgr.list_nodes()?;
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
                let repo = GitRepo::open(&cli.repo)?;
                let mgr = SyncManager::open(&repo)?;
                mgr.retire_node(&uuid)?;
                outln!("retired node {uuid}")?;
            }
        },

        Command::Compact {
            force,
            dry_run,
            no_backup,
            backup_path,
        } => {
            let repo = GitRepo::open(&cli.repo)?;
            let mgr = SyncManager::open(&repo)?;
            if dry_run {
                let nodes = mgr.list_nodes()?;
                let head = compaction::shared_head(&repo, &nodes)?;
                let temp_dir = cli.repo.join(".crdt/temp");
                let temp_count = if temp_dir.exists() {
                    std::fs::read_dir(&temp_dir)
                        .map(|d| {
                            d.filter_map(|e| e.ok())
                                .filter(|e| e.file_name().to_string_lossy() != ".gitkeep")
                                .count()
                        })
                        .unwrap_or(0)
                } else {
                    0
                };
                outln!("shared head: {:?}", head)?;
                outln!("crdt temp files: {temp_count}")?;
                if no_backup {
                    outln!("backup: skipped")?;
                } else {
                    let bp = backup_path
                        .clone()
                        .unwrap_or_else(|| zdb_core::compaction::default_backup_path(&repo));
                    outln!("backup would write: {}", bp.display())?;
                }
                outln!("(dry run — no changes made)")?;
            } else {
                let opts = zdb_core::types::CompactOptions {
                    force,
                    skip_backup: no_backup,
                    backup_path,
                };
                let report = compaction::compact(&repo, &mgr, &opts)?;
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
        }

        Command::Reindex => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            let report = index.rebuild(&repo)?;
            outln!("indexed {} zettels", report.indexed)?;
            if !report.warnings.is_empty() {
                outln!("{} warning(s)", report.warnings.len())?;
            }
        }

        Command::Fix {
            dry_run,
            verbose,
            migrate,
        } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;

            // Run migrations first if requested
            if migrate {
                let mig_report = zdb_core::consistency::migrate_all(&repo, dry_run)?;
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
                            "would migrate {} fields in {} zettels",
                            mig_fixes,
                            mig_report.files_fixed
                        )?;
                    } else {
                        outln!(
                            "migrated {} fields in {} zettels",
                            mig_fixes,
                            mig_report.files_fixed
                        )?;
                    }
                }
            }

            let report = zdb_core::consistency::fix_all(&repo, &index, dry_run)?;

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
                        "would fix {} issues in {} of {} zettels",
                        total_fixes,
                        report.files_fixed,
                        report.files_scanned
                    )?;
                } else {
                    outln!("no issues found")?;
                }
            } else if total_fixes > 0 {
                outln!(
                    "fixed {} issues in {} zettels",
                    total_fixes,
                    report.files_fixed
                )?;
            } else {
                outln!("no issues found")?;
            }
        }

        Command::Rename { id, new_path } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            let old_path = index.resolve_path(&id)?;
            let report = git_ops::rename_zettel(&repo, &index, &old_path, &new_path)?;
            outln!("{} backlinks updated", report.updated.len())?;
            if !report.unresolvable.is_empty() {
                outln!("unresolvable:")?;
                for u in &report.unresolvable {
                    outln!("  {u}")?;
                }
            }
        }

        Command::Bundle { action } => match action {
            BundleAction::Export {
                target,
                full,
                output,
            } => {
                let repo = GitRepo::open(&cli.repo)?;
                let mgr = SyncManager::open(&repo)?;
                if full {
                    let path = zdb_core::bundle::export_full_bundle(&repo, &mgr, &output)?;
                    outln!("exported full bundle to {}", path.display())?;
                } else if let Some(target_uuid) = target {
                    let path = zdb_core::bundle::export_bundle(&repo, &mgr, &target_uuid, &output)?;
                    outln!("exported delta bundle to {}", path.display())?;
                } else {
                    return Err(zdb_core::error::ZettelError::Validation(
                        "specify --target <uuid> or --full".into(),
                    ));
                }
            }
            BundleAction::Import { path } => {
                let repo = GitRepo::open(&cli.repo)?;
                let mut mgr = SyncManager::open(&repo)?;
                let index = open_index(&cli.repo)?;
                let report = zdb_core::bundle::import_bundle(&repo, &mut mgr, &index, &path)?;
                outln!(
                    "imported: conflicts resolved: {}",
                    report.conflicts_resolved
                )?;
            }
        },

        Command::Attach { id, file } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;

            let bytes = std::fs::read(&file)?;
            let filename = file.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
                zdb_core::error::ZettelError::Validation("invalid filename".into())
            })?;
            let mime = zdb_core::types::AttachmentInfo::mime_from_filename(filename);
            let zid = zdb_core::types::ZettelId(id);
            let info =
                zdb_core::attachments::attach_file(&repo, &index, &zid, filename, &bytes, mime)?;
            outln!(
                "attached {} ({}, {} bytes)",
                info.name,
                info.mime,
                info.size
            )?;
        }

        Command::Detach { id, filename } => {
            let repo = GitRepo::open(&cli.repo)?;
            let index = open_index(&cli.repo)?;
            index.rebuild_if_stale(&repo)?;

            let zid = zdb_core::types::ZettelId(id);
            zdb_core::attachments::detach_file(&repo, &index, &zid, &filename)?;
            outln!("detached {}", filename)?;
        }

        Command::Attachments { id } => {
            let repo = GitRepo::open(&cli.repo)?;
            let zid = zdb_core::types::ZettelId(id);
            let list = zdb_core::attachments::list_attachments(&repo, &zid)?;
            if list.is_empty() {
                outln!("no attachments")?;
            } else {
                for a in &list {
                    outln!("{}\t{}\t{} bytes", a.name, a.mime, a.size)?;
                }
            }
        }

        Command::Get { id } => {
            let repo = GitRepo::open(&cli.repo)?;
            let redb_path = cli.repo.join(".zdb/nosql.redb");
            let ri = zdb_core::nosql::RedbIndex::open(&redb_path)?;
            // Ensure redb is in sync
            ri.rebuild(&repo)?;
            match ri.get(&id)? {
                Some(z) => {
                    let content = parser::serialize(&z);
                    out!("{content}")?;
                }
                None => {
                    eprintln!("not found: {id}");
                    std::process::exit(1);
                }
            }
        }

        Command::Scan { r#type, tag } => {
            let repo = GitRepo::open(&cli.repo)?;
            let redb_path = cli.repo.join(".zdb/nosql.redb");
            let ri = zdb_core::nosql::RedbIndex::open(&redb_path)?;
            ri.rebuild(&repo)?;
            let ids = if let Some(t) = r#type {
                ri.scan_by_type(&t)?
            } else if let Some(t) = tag {
                ri.scan_by_tag(&t)?
            } else {
                return Err(zdb_core::error::ZettelError::Validation(
                    "specify --type or --tag".into(),
                ));
            };
            for id in &ids {
                outln!("{id}")?;
            }
        }

        Command::Backlinks { id } => {
            let repo = GitRepo::open(&cli.repo)?;
            let redb_path = cli.repo.join(".zdb/nosql.redb");
            let ri = zdb_core::nosql::RedbIndex::open(&redb_path)?;
            ri.rebuild(&repo)?;
            let ids = ri.backlinks(&id)?;
            for bl in &ids {
                outln!("{bl}")?;
            }
        }

        Command::Serve {
            port,
            pg_port,
            bind,
            playground,
        } => {
            let repo_path = std::fs::canonicalize(&cli.repo)?;
            let rt = tokio::runtime::Runtime::new().map_err(zdb_core::error::ZettelError::Io)?;
            rt.block_on(async {
                zdb_server::run(
                    repo_path,
                    Some(port),
                    Some(pg_port),
                    Some(&bind),
                    playground,
                )
                .await
                .map_err(zdb_core::error::ZettelError::Io)
            })?;
        }

        Command::Maintenance { action } => match action {
            MaintenanceAction::Run { task } => {
                let repo = GitRepo::open(&cli.repo)?;
                let tasks_slice: Vec<&str>;
                let tasks_opt = match &task {
                    Some(t) => {
                        tasks_slice = vec![t.as_str()];
                        Some(tasks_slice.as_slice())
                    }
                    None => None,
                };
                let report = zdb_core::maintenance::run(&repo.path, tasks_opt)?;
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
                let repo = GitRepo::open(&cli.repo)?;
                match auto {
                    AutoAction::Status => {
                        let config = repo.load_config()?;
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
                        let mut config = repo.load_config()?;
                        config.maintenance.auto_enabled = enabled;
                        let toml_str = toml::to_string_pretty(&config)
                            .map_err(|e| zdb_core::error::ZettelError::Toml(e.to_string()))?;
                        repo.commit_file(
                            ".zetteldb.toml",
                            &toml_str,
                            &format!("maintenance auto {}", if enabled { "on" } else { "off" }),
                        )?;
                        outln!(
                            "auto-maintenance {}",
                            if enabled { "enabled" } else { "disabled" }
                        )?;
                    }
                }
            }
        },

        // Handled in main() before run() is called
        Command::UpdateBin | Command::UpdateCheck => unreachable!(),

        Command::Type { action } => match action {
            TypeAction::Suggest { name } => {
                let repo = GitRepo::open(&cli.repo)?;
                let index = open_index(&cli.repo)?;
                index.rebuild_if_stale(&repo)?;

                let schema = index.infer_schema(&name, &repo)?;
                if schema.columns.is_empty() {
                    eprintln!("no data found for type \"{}\"", name);
                    std::process::exit(1);
                }

                let id = unique_id(&cli.repo);
                let zettel = zdb_core::sql_engine::build_typedef_zettel(&id, &schema);
                out!("{}", parser::serialize(&zettel))?;
            }
            TypeAction::Install { name } => {
                let content =
                    zdb_core::bundled_types::get_bundled_type(&name).ok_or_else(|| {
                        zdb_core::error::ZettelError::SqlEngine(format!(
                            "unknown bundled type \"{name}\". available: {:?}",
                            zdb_core::bundled_types::list_bundled_types()
                        ))
                    })?;

                let repo = GitRepo::open(&cli.repo)?;
                let id = unique_id(&cli.repo);

                // Prepend id to the content
                let full_content = content.replacen("---\n", &format!("---\nid: {}\n", id), 1);

                let path = format!("zettelkasten/_typedef/{}.md", id);
                repo.commit_file(&path, &full_content, &format!("install type {name}"))?;

                let index = open_index(&cli.repo)?;
                let parsed = parser::parse(&full_content, &path)?;
                index.index_zettel(&parsed)?;

                outln!("installed type \"{name}\" as {id}")?;
            }
        },
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_broken_pipe;

    #[test]
    fn detects_broken_pipe_io_error() {
        let err = zdb_core::error::ZettelError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "pipe closed",
        ));
        assert!(is_broken_pipe(&err));
    }

    #[test]
    fn ignores_non_broken_pipe_errors() {
        let err = zdb_core::error::ZettelError::Io(std::io::Error::other("boom"));
        assert!(!is_broken_pipe(&err));
    }
}
