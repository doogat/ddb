//! PRD 00176: REST search filter parity and `PageRequest` pagination.
//!
//! `GET /doogats?q=...` must apply the `type`/`tag` filters (no silent
//! `SearchFilters::default()`), return hits carrying every core
//! `SearchResult` field, and decode `page`/`per_page` through
//! `decode_search_page` into a validated `PageRequest` (1..=10,000, no
//! clamping). The plain list endpoint (`GET /doogats` without `q`) keeps its
//! 1-200 clamp.

use super::*;
use axum::body::Body;
use axum::http::Request;
use ddb_core::app_contract::{PageRequest, SearchCommand};
use ddb_core::service::DoogatService;
use serde_json::Value;
use tower::ServiceExt;

// ── decode_search_page ───────────────────────────────────────────

/// Error half of a REST response, flattened to `(status, code, message)` so
/// `expect`/`unwrap` can print it (`ErrorBody` is not `Debug`).
type DecodeErr = (StatusCode, String, String);

/// Call `decode_search_page` with only the given `page`/`per_page` params.
fn decode_rest_page(page: Option<&str>, per_page: Option<&str>) -> Result<PageRequest, DecodeErr> {
    let mut raw = std::collections::HashMap::new();
    if let Some(p) = page {
        raw.insert("page".to_string(), p.to_string());
    }
    if let Some(pp) = per_page {
        raw.insert("per_page".to_string(), pp.to_string());
    }
    decode_search_page(&raw).map_err(|(status, body)| (status, body.0.error, body.0.message))
}

/// Every bad input is a 400 `VALIDATION_ERROR`, never a clamped page.
fn assert_rest_validation(result: Result<PageRequest, DecodeErr>, what: &str) {
    match result {
        Err((status, code, _)) if status == StatusCode::BAD_REQUEST && code == "VALIDATION_ERROR" => {}
        other => panic!("{what}: expected 400 VALIDATION_ERROR, got {other:?}"),
    }
}

#[test]
fn decode_search_page_defaults_to_first_page_of_default_limit() {
    let page = decode_rest_page(None, None).expect("absent page/per_page must decode");
    assert_eq!(
        page,
        PageRequest {
            limit: PageRequest::DEFAULT_LIMIT,
            offset: 0
        }
    );
}

#[test]
fn decode_search_page_uses_default_limit_when_per_page_absent() {
    let page = decode_rest_page(Some("3"), None).expect("page 3 must decode");
    assert_eq!(
        page,
        PageRequest {
            limit: PageRequest::DEFAULT_LIMIT,
            offset: 2 * PageRequest::DEFAULT_LIMIT
        },
        "offset must be (page - 1) * DEFAULT_LIMIT"
    );
}

#[test]
fn decode_search_page_defaults_page_to_one_when_absent() {
    let page = decode_rest_page(None, Some("25")).expect("per_page 25 must decode");
    assert_eq!(page, PageRequest { limit: 25, offset: 0 });
}

#[test]
fn decode_search_page_offset_is_page_minus_one_times_per_page() {
    let cases = [
        ("1", "10", 10, 0),
        ("2", "10", 10, 10),
        ("3", "20", 20, 40),
        ("5", "7", 7, 28),
        ("1", "1", 1, 0),
    ];
    for (page, per_page, limit, offset) in cases {
        let got = decode_rest_page(Some(page), Some(per_page))
            .unwrap_or_else(|e| panic!("page={page} per_page={per_page} must decode: {e:?}"));
        assert_eq!(
            got,
            PageRequest { limit, offset },
            "page={page} per_page={per_page}"
        );
    }
}

#[test]
fn decode_search_page_honours_per_page_above_old_200_cap() {
    assert_eq!(
        decode_rest_page(None, Some("500")).unwrap(),
        PageRequest { limit: 500, offset: 0 }
    );
    assert_eq!(
        decode_rest_page(Some("2"), Some("500")).unwrap(),
        PageRequest {
            limit: 500,
            offset: 500
        }
    );
    assert_eq!(
        decode_rest_page(Some("1"), Some("10000")).unwrap(),
        PageRequest {
            limit: PageRequest::MAX_LIMIT,
            offset: 0
        },
        "per_page of exactly MAX_LIMIT is in range"
    );
}

#[test]
fn decode_search_page_rejects_page_below_one() {
    assert_rest_validation(decode_rest_page(Some("0"), None), "page 0");
    assert_rest_validation(decode_rest_page(Some("-1"), Some("10")), "page -1");
    let min = i64::MIN.to_string();
    assert_rest_validation(decode_rest_page(Some(&min), Some("10")), "page i64::MIN");
}

#[test]
fn decode_search_page_rejects_non_numeric_page() {
    for raw in ["abc", "", "1.5", "two"] {
        assert_rest_validation(
            decode_rest_page(Some(raw), Some("10")),
            &format!("page {raw:?}"),
        );
    }
}

#[test]
fn decode_search_page_rejects_non_numeric_per_page() {
    for raw in ["abc", "", "2.5", "ten"] {
        assert_rest_validation(
            decode_rest_page(Some("1"), Some(raw)),
            &format!("per_page {raw:?}"),
        );
    }
}

#[test]
fn decode_search_page_rejects_per_page_over_max_instead_of_clamping() {
    assert_rest_validation(decode_rest_page(None, Some("10001")), "per_page 10,001");
    assert_rest_validation(decode_rest_page(Some("1"), Some("999999")), "per_page 999,999");
}

#[test]
fn decode_search_page_rejects_non_positive_per_page_instead_of_clamping() {
    assert_rest_validation(decode_rest_page(None, Some("0")), "per_page 0");
    assert_rest_validation(decode_rest_page(Some("1"), Some("-5")), "per_page -5");
}

#[test]
fn decode_search_page_rejects_offset_overflow() {
    let max = i64::MAX.to_string();
    assert_rest_validation(
        decode_rest_page(Some(&max), Some("10000")),
        "(i64::MAX - 1) * 10,000 overflows",
    );
}

// ── GET /doogats?q=... through the router ───────────────────────

fn setup_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let svc = DoogatService::init(dir.path()).unwrap();
    svc.reindex().unwrap();
    let path = dir.path().to_path_buf();
    (dir, path)
}

/// Three doogats sharing `restterm`, differing in type, tags and fields.
/// Returns (soup_id, cake_id, note_id).
fn seed_fixtures(path: &std::path::Path) -> (String, String, String) {
    let svc = DoogatService::open(path).unwrap();
    let mut soup_extra = BTreeMap::new();
    soup_extra.insert("cuisine".to_string(), DdbValue::String("thai".into()));
    let soup = svc
        .create_doogat_with_extra(
            "Soup",
            &["dinner".to_string()],
            Some("recipe"),
            "restterm soup",
            soup_extra,
        )
        .unwrap()
        .meta
        .id
        .expect("created soup must carry an id")
        .0;
    let cake = svc
        .create_doogat(
            "Cake",
            &["dinner".to_string(), "sweet".to_string()],
            Some("dessert"),
            "restterm cake",
        )
        .unwrap();
    let note = svc
        .create_doogat("Note", &[], None, "restterm note")
        .unwrap();
    svc.reindex().unwrap();
    (soup, cake, note)
}

async fn get_json(pool: &ReadPool, uri: &str) -> (StatusCode, Value) {
    let app = router().layer(Extension(pool.clone()));
    let resp = app
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: Value = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{uri}: body is not JSON ({e}): {bytes:?}"));
    (status, json)
}

fn data_array<'a>(body: &'a Value, uri: &str) -> &'a Vec<Value> {
    body["data"]
        .as_array()
        .unwrap_or_else(|| panic!("{uri}: response missing data array: {body}"))
}

fn sorted_ids(hits: &[Value]) -> Vec<String> {
    let mut ids: Vec<String> = hits
        .iter()
        .map(|h| h["id"].as_str().expect("hit missing id").to_string())
        .collect();
    ids.sort();
    ids
}

fn assert_validation_response(status: StatusCode, body: &Value, uri: &str) {
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "{uri}: must be rejected, not clamped; body: {body}"
    );
    assert_eq!(
        body["error"], "VALIDATION_ERROR",
        "{uri}: error code must be VALIDATION_ERROR, got: {body}"
    );
}

#[tokio::test]
async fn rest_search_type_filter_returns_only_matching_hits_with_full_shape() {
    let (_dir, path) = setup_repo();
    let (soup, _cake, _note) = seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    // Fixture sanity through the core contract: the recipe hit carries the
    // `cuisine` field, so a REST hit missing it is a REST defect.
    let core = pool
        .search_command(SearchCommand {
            query: "restterm".into(),
            page: PageRequest {
                limit: 10,
                offset: 0,
            },
            types: Some(vec!["recipe".into()]),
            tag: None,
        })
        .await
        .unwrap();
    assert_eq!(core.value.items.len(), 1, "fixture: one recipe hit");
    assert_eq!(
        core.value.items[0]
            .fields
            .as_ref()
            .and_then(|f| f.get("cuisine"))
            .map(String::as_str),
        Some("thai"),
        "fixture: core hit must carry the cuisine field"
    );

    let uri = "/doogats?q=restterm&type=recipe";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    let hits = data_array(&body, uri);
    assert_eq!(
        sorted_ids(hits),
        vec![soup.clone()],
        "type filter must keep only the recipe hit, got: {body}"
    );

    let hit = hits[0].as_object().expect("hit must be a JSON object");
    for key in [
        "id",
        "title",
        "path",
        "snippet",
        "rank",
        "updated_at",
        "tags",
        "type",
        "fields",
        "created_at",
    ] {
        assert!(
            hit.contains_key(key),
            "search hit must carry `{key}` (core SearchResult field), got: {}",
            hits[0]
        );
    }
    assert_eq!(hits[0]["title"], "Soup");
    assert_eq!(hits[0]["type"], "recipe", "doogat_type serializes as `type`");
    assert_eq!(hits[0]["tags"], serde_json::json!(["dinner"]));
    assert_eq!(hits[0]["fields"]["cuisine"], "thai");
    assert!(
        hits[0]["path"].as_str().is_some_and(|p| p.contains(&soup)),
        "path must name the doogat file, got: {}",
        hits[0]
    );
    assert!(
        hits[0]["created_at"]
            .as_str()
            .is_some_and(|c| !c.is_empty()),
        "created_at must carry the doogat date, got: {}",
        hits[0]
    );
}

#[tokio::test]
async fn rest_search_tag_filter_returns_only_tagged_hits() {
    let (_dir, path) = setup_repo();
    let (_soup, cake, _note) = seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    let uri = "/doogats?q=restterm&tag=sweet";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    let hits = data_array(&body, uri);
    assert_eq!(
        sorted_ids(hits),
        vec![cake],
        "tag filter must keep only the sweet hit, got: {body}"
    );
    let tags = hits[0]["tags"]
        .as_array()
        .unwrap_or_else(|| panic!("hit must carry a tags array, got: {}", hits[0]));
    assert!(
        tags.iter().any(|t| t == "sweet"),
        "tagged hit must list the filter tag, got: {}",
        hits[0]
    );
}

#[tokio::test]
async fn rest_search_type_and_tag_filters_both_apply() {
    let (_dir, path) = setup_repo();
    let (_soup, cake, _note) = seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    let uri = "/doogats?q=restterm&type=dessert&tag=dinner";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert_eq!(sorted_ids(data_array(&body, uri)), vec![cake]);

    // Soup is a recipe but not sweet: both filters must hold at once.
    let uri = "/doogats?q=restterm&type=recipe&tag=sweet";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert!(
        data_array(&body, uri).is_empty(),
        "no hit is both a recipe and sweet, got: {body}"
    );
}

#[tokio::test]
async fn rest_search_type_filter_matching_nothing_returns_empty_hits() {
    let (_dir, path) = setup_repo();
    seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    // Without a filter the query matches all three doogats.
    let (_, unfiltered) = get_json(&pool, "/doogats?q=restterm").await;
    assert_eq!(
        data_array(&unfiltered, "/doogats?q=restterm").len(),
        3,
        "fixture: the unfiltered query matches every doogat"
    );

    let uri = "/doogats?q=restterm&type=nosuchtype";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert!(
        data_array(&body, uri).is_empty(),
        "a type filter matching nothing must return no hits, not the unfiltered set; got: {body}"
    );
}

#[tokio::test]
async fn rest_search_rejects_per_page_over_max() {
    let (_dir, path) = setup_repo();
    seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    for uri in [
        "/doogats?q=restterm&per_page=10001",
        "/doogats?q=restterm&page=1&per_page=999999",
    ] {
        let (status, body) = get_json(&pool, uri).await;
        assert_validation_response(status, &body, uri);
    }
}

#[tokio::test]
async fn rest_search_rejects_non_positive_page() {
    let (_dir, path) = setup_repo();
    seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    for uri in [
        "/doogats?q=restterm&page=0",
        "/doogats?q=restterm&page=-1&per_page=10",
    ] {
        let (status, body) = get_json(&pool, uri).await;
        assert_validation_response(status, &body, uri);
    }
}

#[tokio::test]
async fn rest_search_rejects_non_numeric_page_params() {
    let (_dir, path) = setup_repo();
    seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    for uri in [
        "/doogats?q=restterm&page=abc",
        "/doogats?q=restterm&per_page=lots",
    ] {
        let (status, body) = get_json(&pool, uri).await;
        assert_validation_response(status, &body, uri);
    }
}

#[tokio::test]
async fn rest_search_pages_by_offset_of_page_minus_one_times_per_page() {
    let (_dir, path) = setup_repo();
    seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    let first_uri = "/doogats?q=restterm&page=1&per_page=2";
    let (status, first) = get_json(&pool, first_uri).await;
    assert_eq!(status, StatusCode::OK, "{first_uri}: {first}");
    let second_uri = "/doogats?q=restterm&page=2&per_page=2";
    let (status, second) = get_json(&pool, second_uri).await;
    assert_eq!(status, StatusCode::OK, "{second_uri}: {second}");

    let first_hits = data_array(&first, first_uri);
    let second_hits = data_array(&second, second_uri);
    assert_eq!(first_hits.len(), 2, "per_page 2 caps page 1");
    assert_eq!(second_hits.len(), 1, "page 2 starts at offset 2");
    let mut all = sorted_ids(first_hits);
    all.extend(sorted_ids(second_hits));
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 3, "the two pages must not overlap");
}

/// `count` doogats with explicit past ids (the minting path allows one id per
/// second, so the API create path would take minutes).
fn seed_bulk(path: &std::path::Path, count: usize) {
    let svc = DoogatService::open(path).unwrap();
    for i in 0..count {
        let id = format!(
            "20200101{:02}{:02}{:02}",
            i / 3600,
            (i / 60) % 60,
            i % 60
        );
        let raw =
            format!("---\nid: {id}\ntitle: Bulk {i}\ndate: 2020-01-01\n---\nbulkterm entry {i}\n");
        svc.create_doogat_raw(&raw, "bulk seed").unwrap();
    }
    svc.reindex().unwrap();
}

#[tokio::test]
async fn rest_search_honours_per_page_500_without_clamping_to_200() {
    let (_dir, path) = setup_repo();
    seed_bulk(&path, 201);
    let pool = ReadPool::new(path, 2).unwrap();

    let core = pool
        .search_command(SearchCommand {
            query: "bulkterm".into(),
            page: PageRequest {
                limit: 500,
                offset: 0,
            },
            types: None,
            tag: None,
        })
        .await
        .unwrap();
    assert_eq!(core.value.items.len(), 201, "fixture: 201 bulk hits");

    let uri = "/doogats?q=bulkterm&per_page=500";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert_eq!(
        data_array(&body, uri).len(),
        201,
        "per_page 500 is within the 10,000 ceiling and must not clamp to 200"
    );
}

// ── GET /doogats without q: list endpoint keeps its 1-200 clamp ─

#[tokio::test]
async fn rest_list_without_q_keeps_existing_page_and_per_page_clamp() {
    let (_dir, path) = setup_repo();
    seed_fixtures(&path);
    let pool = ReadPool::new(path, 2).unwrap();

    let uri = "/doogats?per_page=500";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert_eq!(body["pagination"]["per_page"], 200, "list clamps per_page to 200");
    assert_eq!(data_array(&body, uri).len(), 3);

    let uri = "/doogats?per_page=0";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert_eq!(body["pagination"]["per_page"], 1, "list clamps per_page to 1");

    let uri = "/doogats?page=0";
    let (status, body) = get_json(&pool, uri).await;
    assert_eq!(status, StatusCode::OK, "{uri}: {body}");
    assert_eq!(body["pagination"]["page"], 1, "list clamps page to 1");
    assert_eq!(body["pagination"]["total"], 3);
}
