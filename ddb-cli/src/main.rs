use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ddb_core::parser;
use ddb_core::service::DoogatService;
use ddb_core::sql_engine::SqlResult;

mod updater;

const CREATE_APP_GUIDE: &str = "\
CREATE-APP GUIDE
================

Build structured apps on Doogat DB. Define schemas with SQL, query via
GraphQL/CLI/UniFFI. Data lives in Git-backed Markdown with CRDT sync.

1. USE CREATE TABLE, NOT MANUAL TYPEDEFS
----------------------------------------

Define entity schemas with SQL:

  ddb query \"CREATE TABLE bookmark (
    title TEXT NOT NULL,
    url TEXT NOT NULL,
    category TEXT REFERENCES category(id)
  )\"

This creates a _typedef doogat, a materialized SQLite table, and a
GraphQL type. Manual _typedef doogats lack CRDT tracking and may be
flagged by 'ddb fix'.

2. SQL TYPES DETERMINE ZONE PLACEMENT
--------------------------------------

Each column maps to a Markdown zone based on its SQL type:

  frontmatter   INTEGER, REAL, BOOLEAN, CHAR(n), VARCHAR(n<=255),
                TINYTEXT, ENUM, SET
  body          TEXT, VARCHAR(n>255), MEDIUMTEXT, LONGTEXT
  reference     Columns with REFERENCES

3. THREE-ZONE MENTAL MODEL
--------------------------

Every doogat has three zones:

  ---                          <- frontmatter (YAML metadata)
  title: My Record
  type: bookmark
  url: https://example.com
  ---
                               <- body (Markdown content)
  ## Description
  Long-form text goes here.

  ---                          <- references (wikilinks)
  - category:: [[20260301120000]]

Frontmatter holds filterable scalars. Body holds rich text.
References hold links between entities.

4. ENUM AND SET FOR VALUE CONSTRAINTS
-------------------------------------

Constrain column values with ENUM or SET types:

  ddb query \"CREATE TABLE task (
    title TEXT NOT NULL,
    status ENUM('todo','doing','done') DEFAULT 'todo',
    priority ENUM('low','medium','high')
  )\"

ENUM extracts allowed_values into the typedef schema. Values are
validated on INSERT.

5. TITLE RESOLUTION AND title_template
--------------------------------------

Doogat titles are resolved in this order:

  1. Explicit --title on create/update
  2. title_template on the typedef (pattern with {column} placeholders)
  3. The doogat ID as fallback

Set a title template:

  ddb query \"ALTER TABLE contact SET TITLE TEMPLATE '{name}'\"

Remove it:

  ddb query \"ALTER TABLE contact DROP TITLE TEMPLATE\"

6. ZONE OVERRIDES WITH ALTER TABLE SET ZONE
--------------------------------------------

Override the default zone for a column:

  ddb query \"ALTER TABLE bookmark SET ZONE body FOR description\"

This moves the column from frontmatter to body. Existing doogats are
migrated on the next 'ddb fix --migrate'.

7. JUNCTION TABLES FOR MULTI-VALUED REFERENCES
-----------------------------------------------

A REFERENCES column supports multiple values. Each INSERT appends a
reference line:

  ddb query \"INSERT INTO bookmark_category (bookmark_id, category_id)
    VALUES ('20260301120200', '20260301120100')\"

Junction tables ({type}_{column}) are created automatically during
materialization. DELETE removes the reference line.

8. API ACCESS
-------------

CLI:
  ddb query \"SELECT id, title, url FROM bookmark\"
  ddb search \"rust programming\"

GraphQL (ddb serve --port 2891):
  query { bookmarks { id, title, url, category } }
  mutation { executeSql(sql: \"INSERT INTO ...\") { message } }

UniFFI (Swift/Kotlin, embedded):
  let driver = try DoogatDriver.createRepo(repoPath: path)
  try driver.executeSql(\"SELECT name FROM contact\")

9. COMMON MISTAKES
------------------

  * Manual typedefs: Use CREATE TABLE instead. Manual _typedef doogats
    are not CRDT-tracked and will be flagged by 'ddb fix'.

  * Zone surprise: A TEXT column defaults to body, not frontmatter.
    Use VARCHAR(255) for short strings or SET ZONE to override.

  * Title overwrite: Setting --title on a typed doogat overwrites the
    title_template result. Omit --title to let the template work.

Full documentation: docs/src/guide/building-apps.md
";

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

fn write_stdout(args: std::fmt::Arguments<'_>) -> ddb_core::error::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.flush()?;
    Ok(())
}

fn writeln_stdout(args: std::fmt::Arguments<'_>) -> ddb_core::error::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.write_all(b"\n")?;
    stdout.flush()?;
    Ok(())
}

fn is_broken_pipe(err: &ddb_core::error::DoogatError) -> bool {
    matches!(err, ddb_core::error::DoogatError::Io(io_err) if io_err.kind() == io::ErrorKind::BrokenPipe)
}

#[derive(Parser)]
#[command(
    name = "ddb",
    version = env!("DDB_VERSION"),
    about = "Decentralized Doogat DB",
    disable_help_subcommand = true,
    after_help = "GUIDES:\n  help <topic>    In-depth guides (try: ddb help create-app)"
)]
struct Cli {
    /// Repository path (default: current directory)
    #[arg(short, long, default_value = ".")]
    repo: PathBuf,

    /// Directory for NDJSON log files (default: stderr with env filter)
    #[arg(long, global = true, env = "DDB_LOG_DIR")]
    log_dir: Option<PathBuf>,

    /// Log level for ddb crates (trace, debug, info, warn, error)
    #[arg(long, global = true, env = "DDB_LOG_LEVEL")]
    log_level: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new ddb repository
    Init {
        /// Path to create the repository
        path: Option<PathBuf>,
    },
    /// In-depth guides
    Help {
        /// Guide topic (e.g. create-app)
        topic: Option<String>,
    },
    /// Create a new doogat
    #[command(
        after_long_help = "Note: for type definitions, prefer CREATE TABLE via 'ddb query'. See: ddb help create-app"
    )]
    Create {
        #[arg(long)]
        title: String,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        body: Option<String>,
        /// Set a frontmatter field (repeatable, format: key=value)
        #[arg(long, allow_hyphen_values = true)]
        set: Vec<String>,
    },
    /// Read a doogat by ID
    Read {
        /// Doogat ID
        id: String,
    },
    /// Update an existing doogat
    Update {
        /// Doogat ID
        id: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long)]
        tags: Option<String>,
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        #[arg(long, allow_hyphen_values = true)]
        body: Option<String>,
        /// Set a frontmatter field (repeatable, format: key=value)
        #[arg(long, allow_hyphen_values = true)]
        set: Vec<String>,
        /// Remove a frontmatter field (repeatable)
        #[arg(long)]
        unset: Vec<String>,
    },
    /// Delete a doogat by ID
    Delete {
        /// Doogat ID
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
    #[command(
        after_long_help = "Multi-statement strings run in an implicit transaction: if any DML fails, all are rolled back.\n\nFor app data modeling, see: ddb help create-app"
    )]
    Query {
        /// SQL statement (multiple statements separated by ';' are atomic)
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
    /// Auto-fix consistency issues across doogats
    Fix {
        /// Report fixes without modifying files
        #[arg(long)]
        dry_run: bool,
        /// Show detailed fix list per doogat
        #[arg(short, long)]
        verbose: bool,
        /// Run pending field migrations before fixing
        #[arg(long)]
        migrate: bool,
    },
    /// Rename (move) a doogat and rewrite backlinks
    Rename {
        /// Doogat ID
        id: String,
        /// New file path (relative to repo root)
        new_path: String,
    },
    /// Type definition management
    #[command(
        after_long_help = "To define types, use CREATE TABLE via 'ddb query'. See: ddb help create-app"
    )]
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
    /// [experimental] Attach a file to a doogat
    Attach {
        /// Doogat ID
        id: String,
        /// Path to the file to attach
        file: PathBuf,
    },
    /// [experimental] Detach a file from a doogat
    Detach {
        /// Doogat ID
        id: String,
        /// Filename to detach
        filename: String,
    },
    /// [experimental] List attachments on a doogat
    Attachments {
        /// Doogat ID
        id: String,
    },
    /// [experimental] Get doogat by ID via NoSQL index (O(1) lookup)
    Get {
        /// Doogat ID
        id: String,
    },
    /// [experimental] Prefix scan by type or tag via NoSQL index
    Scan {
        /// Filter by doogat type
        #[arg(long, rename_all = "kebab-case")]
        r#type: Option<String>,
        /// Filter by tag
        #[arg(long)]
        tag: Option<String>,
    },
    /// [experimental] List backlinks via NoSQL index
    Backlinks {
        /// Doogat ID
        id: String,
    },
    /// Run git maintenance tasks
    Maintenance {
        #[command(subcommand)]
        action: MaintenanceAction,
    },
    /// [experimental] Update ddb to the latest release
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
    /// Navigate doogat sequences (parent/child chains)
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
    /// Suggest a _typedef doogat from inferred schema
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
    /// Find unlinked mentions of a doogat's title in other doogats
    Mentions {
        /// Doogat ID (omit with --all for summary)
        id: Option<String>,
        /// Show mentions for all doogats
        #[arg(long)]
        all: bool,
    },
    /// Suggest related doogats based on tags and content
    Similar {
        /// Doogat ID
        id: String,
        /// Max suggestions
        #[arg(long, default_value = "10")]
        limit: usize,
    },
    /// Find doogats past their type's staleness threshold
    Stale {
        /// Filter by type
        #[arg(long, name = "type")]
        type_filter: Option<String>,
    },
    /// Find doogats with zero incoming links
    Orphans {
        /// Filter by type
        #[arg(long, name = "type")]
        type_filter: Option<String>,
    },
}

#[derive(Subcommand)]
enum SequenceAction {
    /// Show parent, self, and children of a doogat in a sequence
    Tree {
        /// Doogat ID
        id: String,
    },
    /// Show the path from the sequence root to a doogat
    Breadcrumb {
        /// Doogat ID
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
            "ddb_core={level},ddb_server={level},ddb_cli={level},warn"
        ))
    });

    match log_dir {
        Some(dir) => {
            std::fs::create_dir_all(dir).ok();
            let date = chrono::Local::now().format("%Y-%m-%d");
            let path = dir.join(format!("ddb-{date}.ndjson"));
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

fn parse_set_pairs(
    pairs: &[String],
) -> ddb_core::error::Result<std::collections::BTreeMap<String, ddb_core::types::Value>> {
    let mut map = std::collections::BTreeMap::new();
    for pair in pairs {
        let Some((key, value)) = pair.split_once('=') else {
            return Err(ddb_core::error::DoogatError::Validation(format!(
                "invalid --set format: expected key=value, got '{pair}'"
            )));
        };
        if key.is_empty() {
            return Err(ddb_core::error::DoogatError::Validation(
                "invalid --set format: key cannot be empty".into(),
            ));
        }
        map.insert(
            key.to_string(),
            ddb_core::types::Value::String(value.to_string()),
        );
    }
    Ok(map)
}

fn run(cli: Cli) -> ddb_core::error::Result<()> {
    match cli.command {
        Command::Help { topic } => {
            match topic.as_deref() {
                Some("create-app") => outln!("{CREATE_APP_GUIDE}")?,
                Some(other) => {
                    return Err(ddb_core::error::DoogatError::Validation(
                        format!(
                            "unknown guide: {other}\n\nAvailable guides:\n  create-app    Data modeling, zones, title resolution, and API access"
                        ),
                    ));
                }
                None => {
                    outln!("Available guides:")?;
                    outln!(
                        "  create-app    Data modeling, zones, title resolution, and API access"
                    )?;
                    outln!("")?;
                    outln!("Usage: ddb help <topic>")?;
                }
            }
        }

        Command::Init { path } => {
            let p = path.unwrap_or_else(|| cli.repo.clone());
            DoogatService::init(&p)?;
            outln!("initialized ddb at {}", p.display())?;
        }

        Command::Create {
            title,
            tags,
            r#type,
            body,
            set,
        } => {
            let svc = DoogatService::open(&cli.repo)?;
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
        }

        Command::Read { id } => {
            let svc = DoogatService::open(&cli.repo)?;
            let content = svc.read_doogat(&id)?;
            out!("{content}")?;
        }

        Command::Update {
            id,
            title,
            tags,
            r#type,
            body,
            set,
            unset,
        } => {
            let svc = DoogatService::open(&cli.repo)?;
            let tags_vec: Option<Vec<String>> =
                tags.map(|t| t.split(',').map(|s| s.trim().to_string()).collect());
            let extra = parse_set_pairs(&set)?;
            svc.update_doogat(
                &id,
                title.as_deref(),
                tags_vec.as_deref(),
                r#type.as_deref(),
                body.as_deref(),
                &extra,
                &unset,
            )?;
            outln!("updated {id}")?;
        }

        Command::Delete { id } => {
            let svc = DoogatService::open(&cli.repo)?;
            let broken = svc.delete_doogat(&id, &format!("delete doogat {id}"))?;
            if !broken.is_empty() {
                eprintln!(
                    "warning: {} doogat(s) have broken backlinks after deleting {id}:",
                    broken.len()
                );
                for (src_id, src_path) in &broken {
                    eprintln!("  - {src_id} ({src_path})");
                }
            }
        }

        Command::Sync { remote, branch } => {
            let svc = DoogatService::open(&cli.repo)?;
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
            let mut svc = DoogatService::open(&cli.repo)?;
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
            let svc = DoogatService::open(&cli.repo)?;
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
            let svc = DoogatService::open(&cli.repo)?;
            let node = svc.register_node(&name)?;
            outln!("registered node {} ({})", node.name, node.uuid)?;
        }

        Command::Status => {
            let svc = DoogatService::open(&cli.repo)?;
            let head = svc.head_oid()?;
            let stale = svc.is_index_stale()?;

            let node_uuid = std::fs::read_to_string(cli.repo.join(".git/ddb-node"))
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
        }

        Command::Node { action } => match action {
            NodeAction::List => {
                let svc = DoogatService::open(&cli.repo)?;
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
                let svc = DoogatService::open(&cli.repo)?;
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
            let svc = DoogatService::open(&cli.repo)?;
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
        }

        Command::Reindex => {
            let svc = DoogatService::open(&cli.repo)?;
            let report = svc.reindex()?;
            outln!("indexed {} doogats", report.indexed)?;
            if !report.warnings.is_empty() {
                outln!("{} warning(s)", report.warnings.len())?;
            }
        }

        Command::Fix {
            dry_run,
            verbose,
            migrate,
        } => {
            let svc = DoogatService::open(&cli.repo)?;
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
                let zone_fixes: usize =
                    zone_report.fixes.iter().map(|f| f.applied.len()).sum();
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
        }

        Command::Rename { id, new_path } => {
            let svc = DoogatService::open(&cli.repo)?;
            let report = svc.rename_doogat(&id, &new_path)?;
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
                let svc = DoogatService::open(&cli.repo)?;
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
                let svc = DoogatService::open(&cli.repo)?;
                let report = svc.import_bundle(&path)?;
                outln!(
                    "imported: conflicts resolved: {}",
                    report.conflicts_resolved
                )?;
            }
        },

        Command::Attach { id, file } => {
            let svc = DoogatService::open(&cli.repo)?;
            svc.rebuild_if_stale()?;

            let bytes = std::fs::read(&file)?;
            let filename = file.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
                ddb_core::error::DoogatError::Validation("invalid filename".into())
            })?;
            let mime = ddb_core::types::AttachmentInfo::mime_from_filename(filename);
            let info = svc.attach_file(&id, filename, &bytes, mime)?;
            outln!(
                "attached {} ({}, {} bytes)",
                info.name,
                info.mime,
                info.size
            )?;
        }

        Command::Detach { id, filename } => {
            let svc = DoogatService::open(&cli.repo)?;
            svc.rebuild_if_stale()?;
            svc.detach_file(&id, &filename)?;
            outln!("detached {}", filename)?;
        }

        Command::Attachments { id } => {
            let svc = DoogatService::open(&cli.repo)?;
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
            let svc = DoogatService::open(&cli.repo)?;
            let content = svc.read_doogat(&id)?;
            out!("{content}")?;
        }

        Command::Scan { r#type, tag } => {
            let svc = DoogatService::open(&cli.repo)?;
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
        }

        Command::Backlinks { id } => {
            let svc = DoogatService::open(&cli.repo)?;
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
            let rt = tokio::runtime::Runtime::new().map_err(ddb_core::error::DoogatError::Io)?;
            rt.block_on(async {
                ddb_server::run(
                    repo_path,
                    Some(port),
                    Some(pg_port),
                    Some(&bind),
                    playground,
                )
                .await
                .map_err(ddb_core::error::DoogatError::Io)
            })?;
        }

        Command::Maintenance { action } => match action {
            MaintenanceAction::Run { task } => {
                let svc = DoogatService::open(&cli.repo)?;
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
                let svc = DoogatService::open(&cli.repo)?;
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
            let svc = DoogatService::open(&cli.repo)?;
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
            }
        }

        Command::Sequence { action } => {
            let svc = DoogatService::open(&cli.repo)?;
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
        }

        // Handled in main() before run() is called
        Command::UpdateBin { .. } | Command::UpdateCheck => unreachable!(),

        Command::Type { action } => match action {
            TypeAction::Suggest { name } => {
                let svc = DoogatService::open(&cli.repo)?;
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
                let svc = DoogatService::open(&cli.repo)?;
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
        let err = ddb_core::error::DoogatError::Io(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "pipe closed",
        ));
        assert!(is_broken_pipe(&err));
    }

    #[test]
    fn ignores_non_broken_pipe_errors() {
        let err = ddb_core::error::DoogatError::Io(std::io::Error::other("boom"));
        assert!(!is_broken_pipe(&err));
    }

    // ── parse_set_pairs ─────────────────────────────────────────────

    use ddb_core::types::Value;
    use std::collections::BTreeMap;

    #[test]
    fn parse_set_pairs_single_pair() {
        let pairs = vec!["title=Hello".to_string()];
        let map = super::parse_set_pairs(&pairs).unwrap();
        let mut expected = BTreeMap::new();
        expected.insert("title".to_string(), Value::String("Hello".to_string()));
        assert_eq!(map, expected);
    }

    #[test]
    fn parse_set_pairs_multiple_pairs() {
        let pairs = vec![
            "title=Hello".to_string(),
            "status=active".to_string(),
        ];
        let map = super::parse_set_pairs(&pairs).unwrap();
        assert_eq!(map.len(), 2);
        assert_eq!(map["title"], Value::String("Hello".to_string()));
        assert_eq!(map["status"], Value::String("active".to_string()));
    }

    #[test]
    fn parse_set_pairs_value_containing_equals() {
        let pairs = vec!["url=https://example.com?a=1&b=2".to_string()];
        let map = super::parse_set_pairs(&pairs).unwrap();
        assert_eq!(
            map["url"],
            Value::String("https://example.com?a=1&b=2".to_string()),
        );
    }

    #[test]
    fn parse_set_pairs_empty_value() {
        let pairs = vec!["tag=".to_string()];
        let map = super::parse_set_pairs(&pairs).unwrap();
        assert_eq!(map["tag"], Value::String(String::new()));
    }

    #[test]
    fn parse_set_pairs_missing_equals_is_error() {
        let pairs = vec!["noequals".to_string()];
        assert!(super::parse_set_pairs(&pairs).is_err());
    }

    #[test]
    fn parse_set_pairs_empty_key_is_error() {
        let pairs = vec!["=value".to_string()];
        assert!(super::parse_set_pairs(&pairs).is_err());
    }
}
