use super::*;

#[test]
fn junction_table_name_joins_type_and_column() {
    assert_eq!(junction_table_name("link", "category"), "link_category");
}

#[test]
fn junction_table_name_with_different_inputs() {
    assert_eq!(junction_table_name("project", "owner"), "project_owner");
}

#[test]
fn junction_parent_id_column_appends_id_suffix() {
    assert_eq!(junction_parent_id_column("link"), "link_id");
}

#[test]
fn junction_parent_id_column_with_different_type() {
    assert_eq!(junction_parent_id_column("project"), "project_id");
}

#[test]
fn junction_ref_id_column_appends_id_suffix() {
    assert_eq!(junction_ref_id_column("category"), "category_id");
}

#[test]
fn junction_ref_id_column_with_different_column() {
    assert_eq!(junction_ref_id_column("owner"), "owner_id");
}

#[test]
fn junction_table_ddl_regression_output_preserved() {
    let ddl = junction_table_ddl("link", "category");
    assert_eq!(
        ddl,
        "CREATE TABLE IF NOT EXISTS \"link_category\" (\"link_id\" TEXT NOT NULL, \"category_id\" TEXT NOT NULL, PRIMARY KEY (\"link_id\", \"category_id\"))"
    );
}
