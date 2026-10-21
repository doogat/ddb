use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ddb_core::app_contract::ApplySchemaCommand;
use ddb_core::service::DoogatService;
use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use ddb_server::read_pool::ReadPool;

/// Three typedefs in one repo, so every typedef must reach the SDL, not just
/// the first two. Together they carry every schema trait that shapes the SDL:
/// BOOLEAN / REAL / DATE columns, a `singleton: true` type, a `references:`
/// column, and `required` / `allowed_values`. `field-note` is hyphenated and
/// does not share the `sdl` prefix, so a renderer must not key off type names.
const THREE_TYPES_YAML: &str = "types:
  - name: sdlwidget
    columns:
      - name: widgetlabel
        data_type: VARCHAR(255)
        zone: frontmatter
  - name: sdlgadget
    columns:
      - name: gadgetcount
        data_type: INTEGER
        zone: frontmatter
      - name: gadgetactive
        data_type: BOOLEAN
        zone: frontmatter
      - name: gadgetweight
        data_type: REAL
        zone: frontmatter
      - name: gadgetdue
        data_type: DATE
        zone: frontmatter
  - name: field-note
    singleton: true
    columns:
      - name: notestatus
        data_type: TEXT
        zone: frontmatter
        required: true
        allowed_values:
          - draft
          - final
      - name: noteowner
        data_type: VARCHAR(255)
        references: sdlwidget
";

/// Near-miss pair: `cogtoothpitch` shares the `cogtooth` prefix, so a substring
/// or prefix match would wrongly count one repo's field as the other's.
const COG_YAML: &str = "types:\n  - name: sdlcog\n    columns:\n      - name: cogtooth\n        data_type: VARCHAR(255)\n        zone: frontmatter\n";
const COG_PITCH_YAML: &str = "types:\n  - name: sdlcogpitch\n    columns:\n      - name: cogtoothpitch\n        data_type: VARCHAR(255)\n        zone: frontmatter\n";
/// `COG_YAML` plus one new column: the schema change a stale cache would miss.
const COG_RIM_YAML: &str = "types:\n  - name: sdlcog\n    columns:\n      - name: cogtooth\n        data_type: VARCHAR(255)\n        zone: frontmatter\n      - name: cogrim\n        data_type: INTEGER\n        zone: frontmatter\n";

const FIXTURE_TOKENS: [&str; 4] = ["widgetlabel", "gadgetcount", "cogtooth", "cogtoothpitch"];

/// Apply `yaml` to the repo at `dir` through the core service.
fn apply_schema(dir: &Path, yaml: &str) {
    let mut svc = DoogatService::open(dir).expect("open repo");
    svc.apply_schema(ApplySchemaCommand {
        schema_doc: yaml.to_string(),
        dry_run: false,
        allow_destructive: false,
    })
    .expect("apply fixture schema");
}

/// Init a repo at `dir` and apply `yaml` (if any) through the core service.
fn init_repo(dir: &Path, yaml: Option<&str>) {
    DoogatService::init(dir).expect("init repo");
    if let Some(schema_doc) = yaml {
        apply_schema(dir, schema_doc);
    }
}

/// Call `render_sdl` with a deadline: an implementation that binds a listener
/// and serves would never return, so the timeout turns that into a failure.
async fn render(dir: &Path) -> Result<String, String> {
    tokio::time::timeout(
        Duration::from_secs(60),
        ddb_server::schema::render_sdl(dir.to_path_buf()),
    )
    .await
    .expect("render_sdl must return without serving (timed out)")
}

/// SDL of the live server's schema, built by the same public constructor the
/// server uses, over the repo's current typedefs.
async fn live_sdl(dir: &Path) -> String {
    let type_schemas = DoogatService::open(dir)
        .expect("open repo")
        .list_type_schemas()
        .expect("list type schemas");
    let actor = ActorHandle::spawn(
        dir.to_path_buf(),
        EventBus::new(),
        ddb_core::git_ops::DEFAULT_REMOTE_TIMEOUT,
    )
    .expect("spawn actor");
    let pool = ReadPool::new(dir.to_path_buf(), 1).expect("read pool");
    ddb_server::schema::build_schema(actor, pool, type_schemas, None)
        .expect("live schema must build")
        .sdl()
}

/// Whole identifiers in the SDL, so `cogtooth` never matches `cogtoothpitch`.
fn identifiers(sdl: &str) -> BTreeSet<String> {
    sdl.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect()
}

fn assert_valid_sdl(sdl: &str) {
    if let Err(e) = async_graphql::parser::parse_schema(sdl) {
        panic!("render_sdl must return parseable SDL: {e}\n{sdl}");
    }
    assert!(sdl.contains("type Query"), "SDL must define root Query:\n{sdl}");
    assert!(sdl.contains("type Mutation"), "SDL must define root Mutation:\n{sdl}");
}

/// Every working-tree file and its bytes, outside `.git/` and the derived
/// `.ddb/` index dir. A read-only render must leave this map unchanged.
fn worktree_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).expect("list dir") {
            let path = entry.expect("dir entry").path();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            if rel == Path::new(".git") || rel == Path::new(".ddb") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                out.insert(rel, std::fs::read(&path).expect("read file"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn head(dir: &Path) -> String {
    ddb_core::git_ops::GitRepo::open(dir)
        .expect("open git repo")
        .head_oid()
        .expect("read HEAD")
        .0
}

/// Number of socket fds this process holds. Nothing else in this test binary
/// opens sockets, so a rise across a render means the renderer left one open.
#[cfg(unix)]
fn open_socket_fds() -> usize {
    use std::os::unix::fs::FileTypeExt;
    let fd_dir = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    std::fs::read_dir(fd_dir)
        .expect("list process fds")
        .filter_map(|e| e.ok())
        .filter(|e| {
            std::fs::metadata(e.path())
                .map(|m| m.file_type().is_socket())
                .unwrap_or(false)
        })
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn render_sdl_matches_live_server_schema_for_all_typedefs() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path(), Some(THREE_TYPES_YAML));
    let files_before = worktree_snapshot(tmp.path());
    let head_before = head(tmp.path());

    let sdl = render(tmp.path()).await.expect("render_sdl must succeed");

    // Read-only: no file written into the repo, no commit made.
    assert_eq!(
        worktree_snapshot(tmp.path()),
        files_before,
        "render_sdl must not write into the repo"
    );
    assert_eq!(head(tmp.path()), head_before, "render_sdl must not move HEAD");

    assert_valid_sdl(&sdl);
    let ids = identifiers(&sdl);
    for field in [
        "widgetlabel",
        "gadgetcount",
        "gadgetactive",
        "gadgetweight",
        "gadgetdue",
        "notestatus",
        "noteowner",
    ] {
        assert!(ids.contains(field), "SDL must expose field `{field}`:\n{sdl}");
    }
    let lower = sdl.to_lowercase();
    for ty in ["sdlwidget", "sdlgadget"] {
        assert!(lower.contains(ty), "SDL must expose type `{ty}`:\n{sdl}");
    }
    assert!(ids.contains("FieldNote"), "SDL must expose hyphenated type `field-note`:\n{sdl}");

    // Column data types survive: BOOLEAN and REAL keep their GraphQL scalars.
    assert!(sdl.contains("gadgetactive: Boolean"), "BOOLEAN column must map to Boolean:\n{sdl}");
    assert!(sdl.contains("gadgetweight: Float"), "REAL column must map to Float:\n{sdl}");
    // The singleton flag survives: only singletons get `update_<name>`.
    assert!(sdl.contains("update_field_note("), "singleton type must get update_field_note:\n{sdl}");
    // The `references:` target survives: only reference columns get `<col>_id`.
    assert!(ids.contains("noteowner_id"), "reference column must get raw noteowner_id:\n{sdl}");

    // Same constructor as the live server: byte-identical SDL.
    assert_eq!(sdl, live_sdl(tmp.path()).await, "offline SDL must equal the live server's SDL");
}

#[tokio::test(flavor = "multi_thread")]
async fn render_sdl_reads_the_repo_it_is_given() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    init_repo(a.path(), Some(COG_YAML));
    init_repo(b.path(), Some(COG_PITCH_YAML));

    let sdl_a = render(a.path()).await.expect("render repo A");
    let sdl_b = render(b.path()).await.expect("render repo B");
    let (ids_a, ids_b) = (identifiers(&sdl_a), identifiers(&sdl_b));

    assert!(ids_a.contains("cogtooth"), "repo A SDL must expose cogtooth:\n{sdl_a}");
    assert!(!ids_a.contains("cogtoothpitch"), "repo A SDL must not expose B's field:\n{sdl_a}");
    assert!(ids_b.contains("cogtoothpitch"), "repo B SDL must expose cogtoothpitch:\n{sdl_b}");
    assert!(!ids_b.contains("cogtooth"), "repo B SDL must not expose A's field:\n{sdl_b}");
    assert_ne!(sdl_a, sdl_b, "different typedefs must render different SDL");
}

#[tokio::test(flavor = "multi_thread")]
async fn render_sdl_reflects_schema_changes_on_the_same_repo() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path(), Some(COG_YAML));

    let first = render(tmp.path()).await.expect("first render");
    assert!(!identifiers(&first).contains("cogrim"), "cogrim is not in the schema yet:\n{first}");

    apply_schema(tmp.path(), COG_RIM_YAML);
    let second = render(tmp.path()).await.expect("second render");

    assert!(
        identifiers(&second).contains("cogrim"),
        "render after a schema change must expose the new field (no stale SDL):\n{second}"
    );
    assert_ne!(first, second, "a schema change must change the rendered SDL");
    assert_eq!(second, live_sdl(tmp.path()).await, "re-rendered SDL must equal the live server's SDL");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn render_sdl_leaves_no_socket_open() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path(), Some(COG_YAML));

    let before = open_socket_fds();
    render(tmp.path()).await.expect("render_sdl must succeed");
    // Give a background task time to bind before counting.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let after = open_socket_fds();

    assert!(
        after <= before,
        "render_sdl must not leave a listener or other socket open: {before} sockets before, {after} after"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn render_sdl_on_repo_without_typedefs_yields_base_sdl() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path(), None);

    let sdl = render(tmp.path())
        .await
        .expect("an empty repo must render base SDL, not an error");
    assert_valid_sdl(&sdl);

    let ids = identifiers(&sdl);
    for token in FIXTURE_TOKENS {
        assert!(!ids.contains(token), "empty repo SDL must not expose `{token}`:\n{sdl}");
    }
    assert_eq!(sdl, live_sdl(tmp.path()).await, "empty-repo SDL must equal the live server's base SDL");
}

#[tokio::test(flavor = "multi_thread")]
async fn render_sdl_surfaces_error_for_path_that_is_not_a_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_name = tmp.path().file_name().unwrap().to_string_lossy().into_owned();

    let not_init = render(tmp.path()).await;
    let not_init_msg = match &not_init {
        Err(msg) => msg.clone(),
        Ok(sdl) => panic!("an uninitialized directory must yield Err, got SDL:\n{sdl}"),
    };
    assert!(
        not_init_msg.contains(&tmp_name),
        "the uninitialized-dir error must name the path ({tmp_name}), got: {not_init_msg}"
    );
    assert!(
        std::fs::read_dir(tmp.path()).unwrap().next().is_none(),
        "render_sdl must not create anything in an uninitialized directory"
    );

    let missing_path = tmp.path().join("does-not-exist");
    let missing = render(&missing_path).await;
    let missing_msg = match &missing {
        Err(msg) => msg.clone(),
        Ok(sdl) => panic!("a missing path must yield Err, got SDL:\n{sdl}"),
    };
    assert!(
        missing_msg.contains("does-not-exist"),
        "the missing-path error must name the path, got: {missing_msg}"
    );
    assert!(!missing_path.exists(), "render_sdl must not create a missing repo path");

    assert_ne!(
        not_init_msg, missing_msg,
        "the two failure causes must not collapse into one message"
    );
}
