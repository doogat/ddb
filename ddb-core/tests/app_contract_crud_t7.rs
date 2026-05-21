use ddb_core::app_contract::{
    BrokenBacklink, CreateCommand, DeleteCommand, DeleteResult, ReadCommand, SearchCommand,
    UpdateCommand,
};
use ddb_core::types::value::Value;
use std::collections::BTreeMap;

// --- CreateCommand ---

#[test]
fn create_command_holds_title_and_tags() {
    let cmd = CreateCommand {
        title: String::from("My Note"),
        tags: vec![String::from("rust"), String::from("test")],
        doogat_type: None,
        body: String::from("body text"),
        fields: BTreeMap::new(),
    };
    assert_eq!(cmd.title, "My Note");
    assert_eq!(cmd.tags, vec!["rust", "test"]);
}

#[test]
fn create_command_holds_optional_doogat_type() {
    let cmd = CreateCommand {
        title: String::from("Typed"),
        tags: Vec::new(),
        doogat_type: Some(String::from("project")),
        body: String::new(),
        fields: BTreeMap::new(),
    };
    assert_eq!(cmd.doogat_type, Some(String::from("project")));
}

#[test]
fn create_command_doogat_type_absent_when_none() {
    let cmd = CreateCommand {
        title: String::from("Untyped"),
        tags: Vec::new(),
        doogat_type: None,
        body: String::new(),
        fields: BTreeMap::new(),
    };
    assert!(cmd.doogat_type.is_none());
}

#[test]
fn create_command_holds_fields_map() {
    let mut fields = BTreeMap::new();
    fields.insert(String::from("priority"), Value::Number(1.0));
    fields.insert(String::from("done"), Value::Bool(false));
    let cmd = CreateCommand {
        title: String::from("With Fields"),
        tags: Vec::new(),
        doogat_type: None,
        body: String::new(),
        fields,
    };
    assert_eq!(cmd.fields[&String::from("priority")], Value::Number(1.0));
    assert_eq!(cmd.fields[&String::from("done")], Value::Bool(false));
}

#[test]
fn create_command_is_cloneable() {
    let cmd = CreateCommand {
        title: String::from("Clone Me"),
        tags: vec![String::from("a")],
        doogat_type: Some(String::from("contact")),
        body: String::from("body"),
        fields: BTreeMap::new(),
    };
    let cmd2 = cmd.clone();
    assert_eq!(cmd2.title, "Clone Me");
    assert_eq!(cmd2.doogat_type, Some(String::from("contact")));
}

#[test]
fn create_command_debug_compiles() {
    let cmd = CreateCommand {
        title: String::from("Debug"),
        tags: Vec::new(),
        doogat_type: None,
        body: String::new(),
        fields: BTreeMap::new(),
    };
    let s = format!("{:?}", cmd);
    assert!(s.contains("CreateCommand"));
}

// --- ReadCommand ---

#[test]
fn read_command_holds_id() {
    let cmd = ReadCommand {
        id: String::from("20240101120000"),
    };
    assert_eq!(cmd.id, "20240101120000");
}

#[test]
fn read_command_is_cloneable() {
    let cmd = ReadCommand {
        id: String::from("20240101120000"),
    };
    let cmd2 = cmd.clone();
    assert_eq!(cmd2.id, "20240101120000");
}

#[test]
fn read_command_debug_compiles() {
    let cmd = ReadCommand {
        id: String::from("20240101120000"),
    };
    let s = format!("{:?}", cmd);
    assert!(s.contains("ReadCommand"));
}

// --- UpdateCommand ---

#[test]
fn update_command_holds_id_and_all_optional_fields() {
    let mut fields = BTreeMap::new();
    fields.insert(String::from("status"), Value::String(String::from("active")));
    let cmd = UpdateCommand {
        id: String::from("20240101120000"),
        title: Some(String::from("Updated Title")),
        tags: Some(vec![String::from("updated")]),
        doogat_type: Some(String::from("project")),
        body: Some(String::from("new body")),
        fields,
    };
    assert_eq!(cmd.id, "20240101120000");
    assert_eq!(cmd.title, Some(String::from("Updated Title")));
    assert_eq!(cmd.tags, Some(vec![String::from("updated")]));
    assert_eq!(cmd.doogat_type, Some(String::from("project")));
    assert_eq!(cmd.body, Some(String::from("new body")));
    assert_eq!(
        cmd.fields[&String::from("status")],
        Value::String(String::from("active"))
    );
}

#[test]
fn update_command_all_optional_fields_absent() {
    let cmd = UpdateCommand {
        id: String::from("20240101120000"),
        title: None,
        tags: None,
        doogat_type: None,
        body: None,
        fields: BTreeMap::new(),
    };
    assert!(cmd.title.is_none());
    assert!(cmd.tags.is_none());
    assert!(cmd.doogat_type.is_none());
    assert!(cmd.body.is_none());
    assert!(cmd.fields.is_empty());
}

#[test]
fn update_command_is_cloneable() {
    let cmd = UpdateCommand {
        id: String::from("20240101120000"),
        title: Some(String::from("Clone")),
        tags: None,
        doogat_type: None,
        body: None,
        fields: BTreeMap::new(),
    };
    let cmd2 = cmd.clone();
    assert_eq!(cmd2.id, "20240101120000");
    assert_eq!(cmd2.title, Some(String::from("Clone")));
}

#[test]
fn update_command_debug_compiles() {
    let cmd = UpdateCommand {
        id: String::from("20240101120000"),
        title: None,
        tags: None,
        doogat_type: None,
        body: None,
        fields: BTreeMap::new(),
    };
    let s = format!("{:?}", cmd);
    assert!(s.contains("UpdateCommand"));
}

// --- DeleteCommand ---

#[test]
fn delete_command_holds_id() {
    let cmd = DeleteCommand {
        id: String::from("20240101120000"),
    };
    assert_eq!(cmd.id, "20240101120000");
}

#[test]
fn delete_command_is_cloneable() {
    let cmd = DeleteCommand {
        id: String::from("20240101120000"),
    };
    let cmd2 = cmd.clone();
    assert_eq!(cmd2.id, "20240101120000");
}

#[test]
fn delete_command_debug_compiles() {
    let cmd = DeleteCommand {
        id: String::from("20240101120000"),
    };
    let s = format!("{:?}", cmd);
    assert!(s.contains("DeleteCommand"));
}

// --- SearchCommand ---

#[test]
fn search_command_holds_query_and_optional_limit_offset() {
    let cmd = SearchCommand {
        query: String::from("rust"),
        limit: Some(10),
        offset: Some(20),
    };
    assert_eq!(cmd.query, "rust");
    assert_eq!(cmd.limit, Some(10));
    assert_eq!(cmd.offset, Some(20));
}

#[test]
fn search_command_limit_and_offset_absent_when_none() {
    let cmd = SearchCommand {
        query: String::from("rust"),
        limit: None,
        offset: None,
    };
    assert!(cmd.limit.is_none());
    assert!(cmd.offset.is_none());
}

#[test]
fn search_command_is_cloneable() {
    let cmd = SearchCommand {
        query: String::from("clone"),
        limit: Some(5),
        offset: None,
    };
    let cmd2 = cmd.clone();
    assert_eq!(cmd2.query, "clone");
    assert_eq!(cmd2.limit, Some(5));
}

#[test]
fn search_command_debug_compiles() {
    let cmd = SearchCommand {
        query: String::from("debug"),
        limit: None,
        offset: None,
    };
    let s = format!("{:?}", cmd);
    assert!(s.contains("SearchCommand"));
}

// --- BrokenBacklink ---

#[test]
fn broken_backlink_holds_source_id_and_path() {
    let bl = BrokenBacklink {
        source_id: String::from("20240101120000"),
        source_path: String::from("ddb/20240101120000.md"),
    };
    assert_eq!(bl.source_id, "20240101120000");
    assert_eq!(bl.source_path, "ddb/20240101120000.md");
}

#[test]
fn broken_backlink_is_cloneable() {
    let bl = BrokenBacklink {
        source_id: String::from("20240101120000"),
        source_path: String::from("ddb/20240101120000.md"),
    };
    let bl2 = bl.clone();
    assert_eq!(bl2.source_id, "20240101120000");
    assert_eq!(bl2.source_path, "ddb/20240101120000.md");
}

#[test]
fn broken_backlink_debug_compiles() {
    let bl = BrokenBacklink {
        source_id: String::from("20240101120000"),
        source_path: String::from("ddb/20240101120000.md"),
    };
    let s = format!("{:?}", bl);
    assert!(s.contains("BrokenBacklink"));
}

// --- DeleteResult ---

#[test]
fn delete_result_holds_broken_backlinks() {
    let result = DeleteResult {
        broken_backlinks: vec![
            BrokenBacklink {
                source_id: String::from("20240101120000"),
                source_path: String::from("ddb/20240101120000.md"),
            },
            BrokenBacklink {
                source_id: String::from("20240202080000"),
                source_path: String::from("ddb/20240202080000.md"),
            },
        ],
    };
    assert_eq!(result.broken_backlinks.len(), 2);
    assert_eq!(result.broken_backlinks[0].source_id, "20240101120000");
    assert_eq!(result.broken_backlinks[1].source_id, "20240202080000");
}

#[test]
fn delete_result_with_no_broken_backlinks() {
    let result = DeleteResult {
        broken_backlinks: Vec::new(),
    };
    assert!(result.broken_backlinks.is_empty());
}

#[test]
fn delete_result_is_cloneable() {
    let result = DeleteResult {
        broken_backlinks: vec![BrokenBacklink {
            source_id: String::from("20240101120000"),
            source_path: String::from("ddb/20240101120000.md"),
        }],
    };
    let result2 = result.clone();
    assert_eq!(result2.broken_backlinks.len(), 1);
    assert_eq!(result2.broken_backlinks[0].source_id, "20240101120000");
}

#[test]
fn delete_result_debug_compiles() {
    let result = DeleteResult {
        broken_backlinks: Vec::new(),
    };
    let s = format!("{:?}", result);
    assert!(s.contains("DeleteResult"));
}
