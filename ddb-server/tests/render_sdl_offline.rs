use std::collections::BTreeSet;
use std::time::Duration;

use ddb_core::app_contract::ApplySchemaCommand;
use ddb_core::service::DoogatService;
use ddb_server::actor::ActorHandle;
use ddb_server::events::EventBus;
use ddb_server::read_pool::ReadPool;

/// Two typedefs in one repo: proves every typedef reaches the SDL, not just the
/// first or the last one.
const TWO_TYPES_YAML: &str = "types:\n  - name: sdlwidget\n    columns:\n      - name: widgetlabel\n        data_type: VARCHAR(255)\n        zone: frontmatter\n  - name: sdlgadget\n    columns:\n      - name: gadgetcount\n        data_type: INTEGER\n        zone: frontmatter\n";

/// Near-miss pair: `cogtoothpitch` shares the `cogtooth` prefix, so a substring
/// or prefix match would wrongly count one repo's field as the other's.
const COG_YAML: &str = "types:\n  - name: sdlcog\n    columns:\n      - name: cogtooth\n        data_type: VARCHAR(255)\n        zone: frontmatter\n";
const COG_PITCH_YAML: &str = "types:\n  - name: sdlcogpitch\n    columns:\n      - name: cogtoothpitch\n        data_type: VARCHAR(255)\n        zone: frontmatter\n";

const FIXTURE_TOKENS: [&str; 4] = ["widgetlabel", "gadgetcount", "cogtooth", "cogtoothpitch"];

/// Init a repo at `dir` and apply `yaml` (if any) through the core service.
fn init_repo(dir: &std::path::Path, yaml: Option<&str>) {
    DoogatService::init(dir).expect("init repo");
    if let Some(schema_doc) = yaml {
        let mut svc = DoogatService::open(dir).expect("open repo");
        svc.apply_schema(ApplySchemaCommand {
            schema_doc: schema_doc.to_string(),
            dry_run: false,
            allow_destructive: false,
        })
        .expect("apply fixture schema");
    }
}

/// Call `render_sdl` with a deadline: an implementation that binds a listener
/// and serves would never return, so the timeout turns that into a failure.
async fn render(dir: &std::path::Path) -> Result<String, String> {
    tokio::time::timeout(
        Duration::from_secs(60),
        ddb_server::schema::render_sdl(dir.to_path_buf()),
    )
    .await
    .expect("render_sdl must return without serving (timed out)")
}

/// SDL of the live server's schema, built by the same public constructor the
/// server uses, over the repo's current typedefs.
async fn live_sdl(dir: &std::path::Path) -> String {
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

#[tokio::test(flavor = "multi_thread")]
async fn render_sdl_matches_live_server_schema_for_all_typedefs() {
    let tmp = tempfile::tempdir().unwrap();
    init_repo(tmp.path(), Some(TWO_TYPES_YAML));

    let sdl = render(tmp.path()).await.expect("render_sdl must succeed");
    assert_valid_sdl(&sdl);

    let ids = identifiers(&sdl);
    for field in ["widgetlabel", "gadgetcount"] {
        assert!(ids.contains(field), "SDL must expose field `{field}`:\n{sdl}");
    }
    let lower = sdl.to_lowercase();
    for ty in ["sdlwidget", "sdlgadget"] {
        assert!(lower.contains(ty), "SDL must expose type `{ty}`:\n{sdl}");
    }

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

    let not_init = render(tmp.path()).await;
    assert!(
        matches!(&not_init, Err(msg) if !msg.is_empty()),
        "an uninitialized directory must yield a non-empty Err, got: {not_init:?}"
    );

    let missing = render(&tmp.path().join("does-not-exist")).await;
    assert!(
        matches!(&missing, Err(msg) if !msg.is_empty()),
        "a missing path must yield a non-empty Err, got: {missing:?}"
    );
}
