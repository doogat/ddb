use ddb_core::app_contract::{CreateResult, ListResult, ReadResult, SearchResult, UpdateResult};
use ddb_core::types::{DoogatMeta, ParsedDoogat};

fn sample_parsed_doogat() -> ParsedDoogat {
    ParsedDoogat {
        meta: DoogatMeta::default(),
        body: String::new(),
        sections: vec![],
        reference_section: String::new(),
        inline_fields: vec![],
        links: vec![],
        body_tags: vec![],
        checkboxes: vec![],
        path: "ddb/20260101000000.md".to_string(),
        updated_at: None,
    }
}

#[test]
fn create_result_carries_parsed_doogat() {
    let doogat = sample_parsed_doogat();
    let result = CreateResult {
        doogat: doogat.clone(),
    };
    assert_eq!(result.doogat.path, doogat.path);
}

#[test]
fn list_result_carries_items_and_total() {
    let result = ListResult {
        items: vec![sample_parsed_doogat()],
        total: 1,
    };
    assert_eq!(result.items.len(), 1);
    assert!(result.total >= result.items.len());
}

#[test]
fn search_result_carries_query_for_diagnostics() {
    let result = SearchResult {
        items: vec![],
        total: 0,
        query: "hello".into(),
    };
    assert_eq!(result.query, "hello");
}

#[test]
fn result_dtos_are_clone_and_debug() {
    let doogat = sample_parsed_doogat();

    let create = CreateResult {
        doogat: doogat.clone(),
    };
    let _cloned = create.clone();
    let _fmt = format!("{:?}", create);

    let read = ReadResult {
        doogat: doogat.clone(),
    };
    let _cloned = read.clone();
    let _fmt = format!("{:?}", read);

    let update = UpdateResult {
        doogat: doogat.clone(),
    };
    let _cloned = update.clone();
    let _fmt = format!("{:?}", update);

    let list = ListResult {
        items: vec![doogat.clone()],
        total: 1,
    };
    let _cloned = list.clone();
    let _fmt = format!("{:?}", list);

    let search = SearchResult {
        items: vec![doogat],
        total: 1,
        query: "test".into(),
    };
    let _cloned = search.clone();
    let _fmt = format!("{:?}", search);
}
