use std::io::{self, Write};
use std::path::PathBuf;

use clap::{Parser, Subcommand};

mod updater;
pub(crate) mod warnings;

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

5. STRICT TYPE AND CONSTRAINT ENFORCEMENT
-----------------------------------------

INSERTs are validated against the typedef before any write:

  * NOT NULL columns reject missing or NULL values.
  * INTEGER / REAL / BOOLEAN reject non-numeric strings (including '').
  * VARCHAR(n) / CHAR(n) reject values longer than n.
  * ENUM / SET reject values outside allowed_values.
  * Composite UNIQUE rejects duplicates across the column set.

Breaking change: title has no silent fallback. If title is NOT NULL and
no title_template is set, INSERT without explicit title is rejected.
Provide --title, declare a title_template, or make title nullable.

6. MULTI-ROW INSERT ATOMICITY
-----------------------------

  ddb query \"INSERT INTO task (title, status)
    VALUES ('a','todo'), ('b','doing'), ('c','bogus')\"

All rows are pre-validated before any write. If any row fails, the
entire batch is rejected and no rows are written. Partial success is
not possible.

7. TITLE RESOLUTION AND title_template
--------------------------------------

Doogat titles are resolved in this order:

  1. Explicit --title on create/update
  2. title_template on the typedef (pattern with {column} placeholders)
  3. The doogat ID as fallback (only when title is nullable)

Set a title template:

  ddb query \"ALTER TABLE contact SET TITLE TEMPLATE '{name}'\"

Remove it:

  ddb query \"ALTER TABLE contact DROP TITLE TEMPLATE\"

8. SCHEMA EVOLUTION WITH ALTER TABLE
------------------------------------

Add, drop, or rename columns on an existing typedef:

  ddb query \"ALTER TABLE task ADD COLUMN tags SET('urgent','blocked')\"
  ddb query \"ALTER TABLE task DROP COLUMN priority\"
  ddb query \"ALTER TABLE task RENAME COLUMN status TO state\"

Change a column's declared type (widen VARCHAR, convert to TEXT, etc.):

  ddb query \"ALTER TABLE link ALTER COLUMN url TYPE TEXT\"
  ddb query \"ALTER TABLE link ALTER COLUMN url TYPE VARCHAR(2048)\"

Widening is metadata-only; narrowing pre-flights existing rows and rejects
with a row-count message if any would exceed the new limit.

Override the default zone for a column:

  ddb query \"ALTER TABLE bookmark SET ZONE body FOR description\"

Existing doogats are migrated on the next 'ddb fix --migrate'.

Override the column matched by '<col>=<val>' substring searches:

  ddb query \"ALTER TABLE category SET SEARCH KEY fqn\"
  ddb query \"ALTER TABLE category DROP SEARCH KEY\"

By default, 'category=Y' substring-matches against category.title. SET
SEARCH KEY redirects the match to a different column on the typedef --
useful when the canonical user-facing identifier is not the leaf title
(e.g. category.fqn = 'work.portals' while category.title = 'Portals',
or article.slug = 'getting-started' while title is the long form).

The chosen column must exist on the typedef and must not be a
REFERENCES column. Validation runs at SET time and rejects with a
clear error otherwise. The change persists in the typedef YAML
(search_key:) and takes effect immediately -- no rebuild required.

9. JUNCTION TABLES FOR MULTI-VALUED REFERENCES
-----------------------------------------------

A REFERENCES column supports multiple values. Each INSERT appends a
reference line:

  ddb query \"INSERT INTO bookmark_category (bookmark_id, category_id)
    VALUES ('20260301120200', '20260301120100')\"

Junction tables ({type}_{column}) are created automatically during
materialization. DELETE removes the reference line.

10. API ACCESS
--------------

CLI:
  ddb query \"SELECT id, title, url FROM bookmark\"
  ddb search \"rust programming\"

GraphQL (ddb serve --port 2891):
  query { bookmarks { id, title, url, category } }
  mutation { executeSql(sql: \"INSERT INTO ...\") { message } }

UniFFI (Swift/Kotlin, embedded):
  let driver = try DoogatDriver.createRepo(repoPath: path)
  try driver.executeSql(\"SELECT name FROM contact\")

11. COMMON MISTAKES
-------------------

  * Manual typedefs: Use CREATE TABLE instead. Manual _typedef doogats
    are not CRDT-tracked and will be flagged by 'ddb fix'.

  * Zone surprise: A TEXT column defaults to body, not frontmatter.
    Use VARCHAR(255) for short strings or SET ZONE to override.

  * Explicit --title overrides title_template. Omit --title when you
    want the template to auto-generate the title.

Full documentation: docs/src/guide/building-apps.md
";

macro_rules! out {
    ($($arg:tt)*) => {
        $crate::write_stdout(format_args!($($arg)*))
    };
}

macro_rules! outln {
    ($($arg:tt)*) => {
        $crate::writeln_stdout(format_args!($($arg)*))
    };
}

mod commands;

pub(crate) fn write_stdout(args: std::fmt::Arguments<'_>) -> ddb_core::error::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_fmt(args)?;
    stdout.flush()?;
    Ok(())
}

pub(crate) fn writeln_stdout(args: std::fmt::Arguments<'_>) -> ddb_core::error::Result<()> {
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
    /// Start GraphQL API server
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
    /// List recently modified doogats
    Recent {
        /// Number of days to look back
        #[arg(long, default_value = "7")]
        days: u32,
        /// Filter by type
        #[arg(long, name = "type")]
        type_filter: Option<String>,
    },
    /// Show inbound/outbound link counts per doogat
    LinkDensity {
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

    if let Err(e) = commands::run(cli) {
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

pub(crate) fn fmt_bytes(b: u64) -> String {
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

pub(crate) fn parse_set_pairs(
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
        let pairs = vec!["title=Hello".to_string(), "status=active".to_string()];
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
