use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use zdb_core::parser;
use zdb_core::service::ZettelService;
use zdb_core::sql_engine::SqlResult;

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

    /// Log level for zdb crates (trace, debug, info, warn, error)
    #[arg(long, global = true, env = "ZDB_LOG_LEVEL")]
    log_level: Option<String>,

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
    UpdateBin {
        /// Restore the previously backed-up binary
        #[arg(long)]
        rollback: bool,
    },
    /// Background update check (internal)
    #[command(name = "__update-check", hide = true)]
    UpdateCheck,
    /// Discover connections and maintenance issues
    Discover {
        #[command(subcommand)]
        action: DiscoverAction,
    },
    /// Navigate zettel sequences (parent/child chains)
    Sequence {
        #[command(subcommand)]
        action: SequenceAction,
    },
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

#[derive(Subcommand)]
enum DiscoverAction {
    /// Find unlinked mentions of a zettel's title in other zettels
    Mentions {
        /// Zettel ID (omit with --all for summary)
        id: Option<String>,
        /// Show mentions for all zettels
        #[arg(long)]
        all: bool,
    },
    /// Suggest related zettels based on tags and content
    Similar {
        /// Zettel ID
        id: String,
        /// Max suggestions
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Find zettels past their type's staleness threshold
    Stale {
        /// Filter by type
        #[arg(long, name = "type")]
        type_filter: Option<String>,
    },
    /// Find zettels with zero incoming links
    Orphans {
        /// Filter by type
        #[arg(long, name = "type")]
        type_filter: Option<String>,
    },
}

#[derive(Subcommand)]
enum SequenceAction {
    /// Show parent, self, and children of a zettel in a sequence
    Tree {
        /// Zettel ID
        id: String,
    },
    /// Show the path from the sequence root to a zettel
    Breadcrumb {
        /// Zettel ID
        id: String,
    },
    /// List broken sequence references (parent doesn't exist)
    Broken,
}

fn main() {
    let cli = Cli::parse();
    init_logging(cli.log_dir.as_deref(), cli.log_level.as_deref());

    // Handle update commands before anything else
    match &cli.command {
        Command::UpdateCheck => {
            updater::check_and_update();
            return;
        }
        Command::UpdateBin { rollback } => {
            let result = if *rollback {
                updater::rollback()
            } else {
                updater::run_update()
            };
            if let Err(e) = result {
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

fn init_logging(log_dir: Option<&std::path::Path>, log_level: Option<&str>) {
    use tracing_subscriber::fmt;
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        let level = log_level.unwrap_or("info");
        EnvFilter::new(format!(
            "zdb_core={level},zdb_server={level},zdb_cli={level},warn"
        ))
    });

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
                        .compact()
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
                .compact()
                .with_writer(std::io::stderr)
                .with_env_filter(filter)
                .init();
        }
    }
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
            ZettelService::init(&p)?;
            outln!("initialized zettelkasten at {}", p.display())?;
        }

        Command::Create {
            title,
            tags,
            r#type,
            body,
        } => {
            let svc = ZettelService::open(&cli.repo)?;
            let tags_list: Vec<String> = tags
                .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
                .unwrap_or_default();
            let body_text = body.unwrap_or_default();

            let id = svc.create_zettel(&title, &tags_list, r#type.as_deref(), &body_text)?;
            outln!("{id}")?;
        }

        Command::Read { id } => {
            let svc = ZettelService::open(&cli.repo)?;
            let content = svc.read_zettel(&id)?;
            out!("{content}")?;
        }

        Command::Update {
            id,
            title,
            tags,
            r#type,
            body,
        } => {
            let svc = ZettelService::open(&cli.repo)?;
            let tags_vec: Option<Vec<String>> =
                tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
            svc.update_zettel(
                &id,
                title.as_deref(),
                tags_vec.as_deref(),
                r#type.as_deref(),
                body.as_deref(),
            )?;
            outln!("updated {id}")?;
        }

        Command::Delete { id } => {
            let svc = ZettelService::open(&cli.repo)?;
            let broken = svc.delete_zettel(&id, &format!("delete zettel {id}"))?;
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
            let svc = ZettelService::open(&cli.repo)?;
            let report = svc.sync(&remote, &branch)?;
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
        }

        Command::Query { sql } => {
            let mut svc = ZettelService::open(&cli.repo)?;
            svc.rebuild_if_stale()?;
            for result in svc.execute_batch(&sql)? {
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
            let svc = ZettelService::open(&cli.repo)?;
            let result = svc.search_paginated(&query, limit, offset)?;
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
            let svc = ZettelService::open(&cli.repo)?;
            let node = svc.register_node(&name)?;
            outln!("registered node {} ({})", node.name, node.uuid)?;
        }

        Command::Status => {
            let svc = ZettelService::open(&cli.repo)?;
            let head = svc.head_oid()?;
            let stale = svc.is_index_stale()?;

            let node_uuid = std::fs::read_to_string(cli.repo.join(".git/zdb-node"))
                .unwrap_or_else(|_| "not registered".into());

            let mut stale_nodes = Vec::new();
            let node_count = if let Ok(nodes) = svc.list_nodes() {
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

            let resurrected = svc.resurrected_zettels().unwrap_or_default();
            if !resurrected.is_empty() {
                outln!("resurrected zettels: {}", resurrected.len())?;
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
        }

        Command::Node { action } => match action {
            NodeAction::List => {
                let svc = ZettelService::open(&cli.repo)?;
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
                let svc = ZettelService::open(&cli.repo)?;
                svc.retire_node(&uuid)?;
                outln!("retired node {uuid}")?;
            }
        },

        Command::Compact {
            force,
            dry_run,
            no_backup,
            backup_path,
        } => {
            let svc = ZettelService::open(&cli.repo)?;
            if dry_run {
                let info = svc.compact_dry_run()?;
                outln!("shared head: {:?}", info.shared_head)?;
                outln!("crdt temp files: {}", info.crdt_temp_files)?;
                if no_backup {
                    outln!("backup: skipped")?;
                } else {
                    let bp = backup_path
                        .clone()
                        .unwrap_or(info.default_backup_path);
                    outln!("backup would write: {}", bp.display())?;
                }
                outln!("(dry run — no changes made)")?;
            } else {
                let opts = zdb_core::types::CompactOptions {
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
        }

        Command::Reindex => {
            let svc = ZettelService::open(&cli.repo)?;
            let report = svc.reindex()?;
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
            let svc = ZettelService::open(&cli.repo)?;
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
            let svc = ZettelService::open(&cli.repo)?;
            let report = svc.rename_zettel(&id, &new_path)?;
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
                let svc = ZettelService::open(&cli.repo)?;
                if full {
                    let path = svc.export_full_bundle(&output)?;
                    outln!("exported full bundle to {}", path.display())?;
                } else if let Some(target_uuid) = target {
                    let path = svc.export_delta_bundle(&target_uuid, &output)?;
                    outln!("exported delta bundle to {}", path.display())?;
                } else {
                    return Err(zdb_core::error::ZettelError::Validation(
                        "specify --target <uuid> or --full".into(),
                    ));
                }
            }
            BundleAction::Import { path } => {
                let svc = ZettelService::open(&cli.repo)?;
                let report = svc.import_bundle(&path)?;
                outln!(
                    "imported: conflicts resolved: {}",
                    report.conflicts_resolved
                )?;
            }
        },

        Command::Attach { id, file } => {
            let svc = ZettelService::open(&cli.repo)?;
            svc.rebuild_if_stale()?;

            let bytes = std::fs::read(&file)?;
            let filename = file.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
                zdb_core::error::ZettelError::Validation("invalid filename".into())
            })?;
            let mime = zdb_core::types::AttachmentInfo::mime_from_filename(filename);
            let info = svc.attach_file(&id, filename, &bytes, mime)?;
            outln!(
                "attached {} ({}, {} bytes)",
                info.name,
                info.mime,
                info.size
            )?;
        }

        Command::Detach { id, filename } => {
            let svc = ZettelService::open(&cli.repo)?;
            svc.rebuild_if_stale()?;
            svc.detach_file(&id, &filename)?;
            outln!("detached {}", filename)?;
        }

        Command::Attachments { id } => {
            let svc = ZettelService::open(&cli.repo)?;
            let list = svc.list_attachments(&id)?;
            if list.is_empty() {
                outln!("no attachments")?;
            } else {
                for a in &list {
                    outln!("{}\t{}\t{} bytes", a.name, a.mime, a.size)?;
                }
            }
        }

        Command::Get { id } => {
            let svc = ZettelService::open(&cli.repo)?;
            let content = svc.read_zettel(&id)?;
            out!("{content}")?;
        }

        Command::Scan { r#type, tag } => {
            let svc = ZettelService::open(&cli.repo)?;
            let ids = if let Some(t) = r#type {
                svc.nosql_scan_type(&t)?
            } else if let Some(t) = tag {
                svc.nosql_scan_tag(&t)?
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
            let svc = ZettelService::open(&cli.repo)?;
            let ids = svc.backlink_ids(&id)?;
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
                let svc = ZettelService::open(&cli.repo)?;
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
                let svc = ZettelService::open(&cli.repo)?;
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
        },

        Command::Discover { action } => {
            let svc = ZettelService::open(&cli.repo)?;
            svc.rebuild_if_stale()?;

            match action {
                DiscoverAction::Mentions { id, all } => {
                    if all {
                        let all_ids = svc.all_zettel_ids()?;
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
                        return Err(zdb_core::error::ZettelError::Validation(
                            "specify a zettel ID or --all".into(),
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
                    let stale = svc.stale_zettels(type_filter.as_deref())?;
                    if stale.is_empty() {
                        outln!("no stale zettels")?;
                    } else {
                        for s in &stale {
                            outln!(
                                "{}\t{}\t{}\t{}\t{}d stale",
                                s.id,
                                s.title,
                                s.zettel_type,
                                s.last_updated,
                                s.days_stale
                            )?;
                        }
                    }
                }
                DiscoverAction::Orphans { type_filter } => {
                    let orphans = svc.orphan_zettels(type_filter.as_deref())?;
                    if orphans.is_empty() {
                        outln!("no orphan zettels")?;
                    } else {
                        for o in &orphans {
                            outln!(
                                "{}\t{}\t{}\t{} outgoing",
                                o.id,
                                o.title,
                                o.zettel_type,
                                o.outgoing_links
                            )?;
                        }
                    }
                }
            }
        }

        Command::Sequence { action } => {
            let svc = ZettelService::open(&cli.repo)?;
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
                            outln!("{} -> {} (not found)", b.zettel_id, b.broken_parent_id)?;
                        }
                    }
                }
            }
        }

        // Handled in main() before run() is called
        Command::UpdateBin { .. } | Command::UpdateCheck => unreachable!(),

        Command::Type { action } => match action {
            TypeAction::Suggest { name } => {
                let svc = ZettelService::open(&cli.repo)?;
                let schema = svc.infer_schema(&name)?;
                if schema.columns.is_empty() {
                    eprintln!("no data found for type \"{}\"", name);
                    std::process::exit(1);
                }

                let id = parser::generate_id();
                let zettel = zdb_core::sql_engine::build_typedef_zettel(&id, &schema);
                out!("{}", parser::serialize(&zettel))?;
            }
            TypeAction::Install { name } => {
                let svc = ZettelService::open(&cli.repo)?;
                let id = svc.install_bundled_type(&name)?;
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
