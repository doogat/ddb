use crate::search_query::{self, SearchExpr};
use crate::types::{SearchFieldOp, SearchFilters};

use super::Index;

/// Escape a SQL identifier by doubling embedded double-quotes.
pub(super) fn escape_sql_ident(name: &str) -> String {
    name.replace('"', "\"\"")
}

impl Index {
    /// Parse the query and extract negation info.
    /// Returns `None` if no negation handling needed (original behavior).
    /// Returns `Some((positive_fts_query, negation_sql_clauses, negation_params))`
    /// where positive_fts_query is None for all-negative queries.
    pub(super) fn build_negation_plan(&self, query: &str) -> Option<(Option<String>, Vec<String>, Vec<String>)> {
        let ast = search_query::parse(query)?;
        let (positive, negatives) = search_query::extract_negations(ast);
        if negatives.is_empty() {
            return None;
        }

        let pos_query = positive.as_ref().map(search_query::to_fts_query);

        let mut neg_clauses = Vec::new();
        let mut neg_params = Vec::new();
        for neg in &negatives {
            match neg {
                SearchExpr::FieldEquals { field, value } if field == "tag" => {
                    neg_clauses.push("tag".to_string());
                    neg_params.push(value.clone());
                }
                other => {
                    neg_clauses.push("fts".to_string());
                    neg_params.push(search_query::to_fts_query(other));
                }
            }
        }

        Some((pos_query, neg_clauses, neg_params))
    }

    /// Generate SQL exclusion clauses for negated terms and collect params.
    pub(super) fn resolve_negation_clauses(
        &self,
        neg_clauses: &[String],
        neg_params: &[String],
        all_params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    ) -> String {
        let mut sql = String::new();
        for (clause_type, value) in neg_clauses.iter().zip(neg_params.iter()) {
            all_params.push(Box::new(value.clone()));
            let idx = all_params.len();
            match clause_type.as_str() {
                "tag" => {
                    sql.push_str(&format!(
                        " AND z.id NOT IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?{idx})"
                    ));
                }
                _ => {
                    sql.push_str(&format!(
                        " AND z.id NOT IN (\
                         SELECT z2.id FROM _ddb_fts \
                         JOIN doogats z2 ON z2.rowid = _ddb_fts.rowid \
                         WHERE _ddb_fts MATCH ?{idx})"
                    ));
                }
            }
        }
        sql
    }

    /// Re-index filter SQL clauses for all-negative queries where there is
    /// no ?1 FTS MATCH param. Copies filter_params into all_params and
    /// returns adjusted SQL with correct parameter indices.
    pub(super) fn reindex_filter_sql(
        filter_sql: &str,
        filter_params: &[String],
        all_params: &mut Vec<Box<dyn rusqlite::types::ToSql>>,
    ) -> String {
        if filter_params.is_empty() {
            return String::new();
        }
        let base = all_params.len();
        for p in filter_params {
            all_params.push(Box::new(p.clone()));
        }
        // Two-pass replacement to avoid overlapping index corruption.
        // Pass 1: old indices -> unique markers
        let mut result = filter_sql.to_string();
        for i in (0..filter_params.len()).rev() {
            let old = format!("?{}", i + 2);
            let marker = format!("__P{i}__");
            result = result.replace(&old, &marker);
        }
        // Pass 2: markers -> new indices
        for i in 0..filter_params.len() {
            let marker = format!("__P{i}__");
            let new = format!("?{}", base + i + 1);
            result = result.replace(&marker, &new);
        }
        result
    }

    pub(super) fn build_filter_clauses(&self, filters: &SearchFilters) -> (Vec<String>, Vec<String>) {
        use super::materialize::is_core_column;

        let mut clauses = Vec::new();
        let mut params = Vec::new();
        let mut idx = 2; // ?1 is always the FTS query

        if let Some(ref types) = filters.types {
            if !types.is_empty() {
                let placeholders: Vec<String> =
                    types.iter().map(|_| { let p = format!("?{idx}"); idx += 1; p }).collect();
                clauses.push(format!("AND z.type IN ({})", placeholders.join(", ")));
                params.extend(types.clone());
            }
        }

        if let Some(ref tag) = filters.tag {
            clauses.push(format!(
                "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?{idx})"
            ));
            params.push(tag.clone());
            idx += 1;
        }

        if let Some(ref where_filters) = filters.where_filters {
            // Determine candidate type tables for materialized column lookup
            let candidate_tables: Vec<String> = if let Some(ref types) = filters.types {
                types.clone()
            } else {
                self.conn
                    .prepare(
                        "SELECT name FROM sqlite_master WHERE type='table' \
                         AND name NOT LIKE '_ddb%' AND name != 'doogats' \
                         AND name NOT LIKE 'sqlite_%'",
                    )
                    .and_then(|mut stmt| {
                        stmt.query_map([], |row| row.get::<_, String>(0))
                            .map(|rows| rows.flatten().collect::<Vec<String>>())
                    })
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|name| {
                        // Exclude junction tables (they have no `id` column)
                        self.conn
                            .prepare(&format!(
                                "PRAGMA table_info(\"{}\")",
                                escape_sql_ident(name)
                            ))
                            .and_then(|mut stmt| {
                                stmt.query_map([], |row| row.get::<_, String>(1))
                                    .map(|rows| rows.flatten().any(|col| col == "id"))
                            })
                            .unwrap_or(false)
                    })
                    .collect::<Vec<String>>()
            };

            for wf in where_filters {
                // Route "tag" field to _ddb_tags table
                if wf.field == "tag" {
                    match &wf.op {
                        SearchFieldOp::Eq(val) => {
                            clauses.push(format!(
                                "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag = ?{idx})"
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::Contains(val) => {
                            clauses.push(format!(
                                "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag LIKE '%' || ?{idx} || '%')"
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::In(vals) => {
                            if vals.is_empty() {
                                clauses.push("AND 0".to_string());
                            } else {
                                let placeholders: Vec<String> = vals.iter().map(|_| {
                                    let p = format!("?{idx}");
                                    idx += 1;
                                    p
                                }).collect();
                                clauses.push(format!(
                                    "AND z.id IN (SELECT doogat_id FROM _ddb_tags WHERE tag IN ({}))",
                                    placeholders.join(", ")
                                ));
                                params.extend(vals.clone());
                            }
                        }
                    }
                    continue;
                }

                // Route core doogat columns directly to the z (doogats) alias
                if is_core_column(&wf.field) {
                    let safe_col = escape_sql_ident(&wf.field);
                    match &wf.op {
                        SearchFieldOp::Eq(val) => {
                            clauses.push(format!(
                                "AND z.\"{}\" = ?{idx}", safe_col
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::Contains(val) => {
                            clauses.push(format!(
                                "AND z.\"{}\" LIKE '%' || ?{idx} || '%'", safe_col
                            ));
                            params.push(val.clone());
                            idx += 1;
                        }
                        SearchFieldOp::In(vals) => {
                            if vals.is_empty() {
                                clauses.push("AND 0".to_string());
                            } else {
                                let placeholders: Vec<String> = vals.iter().map(|_| {
                                    let p = format!("?{idx}");
                                    idx += 1;
                                    p
                                }).collect();
                                clauses.push(format!(
                                    "AND z.\"{}\" IN ({})", safe_col, placeholders.join(", ")
                                ));
                                params.extend(vals.clone());
                            }
                        }
                    }
                    continue;
                }

                // Find which candidate tables have this field as a non-core column
                let mut tables_with_field: Vec<String> = Vec::new();
                for table in &candidate_tables {
                    let has_col = self
                        .conn
                        .prepare(&format!(
                            "PRAGMA table_info(\"{}\")",
                            escape_sql_ident(table)
                        ))
                        .and_then(|mut stmt| {
                            stmt.query_map([], |row| row.get::<_, String>(1))
                                .map(|rows| {
                                    rows.flatten()
                                        .any(|col| !is_core_column(&col) && col == wf.field)
                                })
                        })
                        .unwrap_or(false);
                    if has_col {
                        tables_with_field.push(table.clone());
                    }
                }

                if tables_with_field.is_empty() {
                    // Check for junction tables ({type}_{field}) before
                    // falling back to _ddb_fields.
                    let safe_field = escape_sql_ident(&wf.field);
                    let mut junction_tables: Vec<String> = Vec::new();
                    for table in &candidate_tables {
                        let jt_name = format!("{}_{}", table, wf.field);
                        let exists: bool = self
                            .conn
                            .query_row(
                                "SELECT COUNT(*) > 0 FROM sqlite_master \
                                 WHERE type='table' AND name=?1",
                                [&jt_name],
                                |row| row.get(0),
                            )
                            .unwrap_or(false);
                        if exists {
                            junction_tables.push(table.clone());
                        }
                    }

                    if !junction_tables.is_empty() {
                        match &wf.op {
                            SearchFieldOp::Eq(val) => {
                                let ph = format!("?{idx}");
                                idx += 1;
                                let subs: Vec<String> = junction_tables
                                    .iter()
                                    .map(|t| {
                                        let st = escape_sql_ident(t);
                                        let jt = format!("{st}_{safe_field}");
                                        format!(
                                            "SELECT \"{st}_id\" FROM \"{jt}\" \
                                             WHERE \"{safe_field}_id\" = {ph}"
                                        )
                                    })
                                    .collect();
                                clauses.push(format!(
                                    "AND z.id IN ({})",
                                    subs.join(" UNION ")
                                ));
                                params.push(val.clone());
                            }
                            SearchFieldOp::Contains(val) => {
                                let ph = format!("?{idx}");
                                idx += 1;
                                let subs: Vec<String> = junction_tables
                                    .iter()
                                    .map(|t| {
                                        let st = escape_sql_ident(t);
                                        let jt = format!("{st}_{safe_field}");
                                        format!(
                                            "SELECT jt.\"{st}_id\" FROM \"{jt}\" jt \
                                             JOIN doogats d ON d.id = jt.\"{safe_field}_id\" \
                                             WHERE d.title LIKE '%' || {ph} || '%'"
                                        )
                                    })
                                    .collect();
                                clauses.push(format!(
                                    "AND z.id IN ({})",
                                    subs.join(" UNION ")
                                ));
                                params.push(val.clone());
                            }
                            SearchFieldOp::In(vals) => {
                                if vals.is_empty() {
                                    clauses.push("AND 0".to_string());
                                } else {
                                    let phs: Vec<String> = vals
                                        .iter()
                                        .map(|_| {
                                            let p = format!("?{idx}");
                                            idx += 1;
                                            p
                                        })
                                        .collect();
                                    let in_list = phs.join(", ");
                                    let subs: Vec<String> = junction_tables
                                        .iter()
                                        .map(|t| {
                                            let st = escape_sql_ident(t);
                                            let jt = format!("{st}_{safe_field}");
                                            format!(
                                                "SELECT \"{st}_id\" FROM \"{jt}\" \
                                                 WHERE \"{safe_field}_id\" IN ({in_list})"
                                            )
                                        })
                                        .collect();
                                    clauses.push(format!(
                                        "AND z.id IN ({})",
                                        subs.join(" UNION ")
                                    ));
                                    params.extend(vals.clone());
                                }
                            }
                        }
                    } else {
                        // Fallback: use _ddb_fields key-value store (two params)
                        match &wf.op {
                            SearchFieldOp::Eq(val) => {
                                clauses.push(format!(
                                    "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value = ?{})",
                                    idx, idx + 1
                                ));
                                params.push(wf.field.clone());
                                params.push(val.clone());
                                idx += 2;
                            }
                            SearchFieldOp::Contains(val) => {
                                clauses.push(format!(
                                    "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value LIKE '%' || ?{} || '%')",
                                    idx, idx + 1
                                ));
                                params.push(wf.field.clone());
                                params.push(val.clone());
                                idx += 2;
                            }
                            SearchFieldOp::In(vals) => {
                                if vals.is_empty() {
                                    clauses.push("AND 0".to_string());
                                } else {
                                    let key_idx = idx;
                                    idx += 1;
                                    let placeholders: Vec<String> = vals.iter().map(|_| {
                                        let p = format!("?{idx}");
                                        idx += 1;
                                        p
                                    }).collect();
                                    clauses.push(format!(
                                        "AND z.id IN (SELECT doogat_id FROM _ddb_fields WHERE key = ?{} AND value IN ({}))",
                                        key_idx, placeholders.join(", ")
                                    ));
                                    params.push(wf.field.clone());
                                    params.extend(vals.clone());
                                }
                            }
                        }
                    } // end junction_tables else (fallback)
                } else {
                    // Resolve against materialized type table(s)
                    let safe_col = escape_sql_ident(&wf.field);

                    match &wf.op {
                        SearchFieldOp::In(vals) => {
                            if vals.is_empty() {
                                clauses.push("AND 0".to_string());
                            } else {
                                let placeholders: Vec<String> = vals.iter().map(|_| {
                                    let p = format!("?{idx}");
                                    idx += 1;
                                    p
                                }).collect();
                                let in_list = placeholders.join(", ");

                                let subqueries: Vec<String> = tables_with_field
                                    .iter()
                                    .map(|t| {
                                        let safe_table = escape_sql_ident(t);
                                        format!(
                                            "SELECT id FROM \"{}\" WHERE \"{}\" IN ({})",
                                            safe_table, safe_col, in_list
                                        )
                                    })
                                    .collect();

                                let combined = subqueries.join(" UNION ");
                                clauses.push(format!("AND z.id IN ({combined})"));
                                params.extend(vals.clone());
                            }
                        }
                        SearchFieldOp::Contains(val) => {
                            // PRD 00133: resolution priority for REFERENCES
                            // columns where the parent type has the column
                            // materialized:
                            //   1. If a typedef table named `<col>` exists,
                            //      resolve <val> against that table's title
                            //      with LIKE. This is the new preferred path
                            //      because `INSERT INTO link (category)
                            //      VALUES (...)` populates the materialized
                            //      column directly but does NOT populate the
                            //      auto-junction `link_category` (that only
                            //      happens during full rebuild), so a
                            //      junction-based query would return 0 rows
                            //      on fresh data.
                            //   2. Else if the auto-junction
                            //      `<type>_<col>` exists AND has rows, JOIN
                            //      through it on referenced doogat title.
                            //      This is the legacy path; it stays for
                            //      callers / tests that pre-populate the
                            //      junction (e.g. by going through a full
                            //      rebuild, or by manual setup).
                            //   3. Else fall back to direct column LIKE on
                            //      the materialized column.
                            let ref_table_exists = self
                                .conn
                                .query_row(
                                    "SELECT COUNT(*) > 0 FROM sqlite_master \
                                     WHERE type='table' AND name=?1",
                                    [&wf.field],
                                    |row| row.get::<_, bool>(0),
                                )
                                .unwrap_or(false);

                            let jt_tables: Vec<String> = if ref_table_exists {
                                Vec::new()
                            } else {
                                tables_with_field
                                    .iter()
                                    .filter(|t| {
                                        let jt_name = format!("{}_{}", t, wf.field);
                                        self.conn
                                            .query_row(
                                                "SELECT COUNT(*) > 0 FROM sqlite_master \
                                                 WHERE type='table' AND name=?1",
                                                [&jt_name],
                                                |row| row.get::<_, bool>(0),
                                            )
                                            .unwrap_or(false)
                                    })
                                    .cloned()
                                    .collect()
                            };

                            if ref_table_exists {
                                let ph = format!("?{idx}");
                                idx += 1;
                                let safe_ref = escape_sql_ident(&wf.field);
                                let subs: Vec<String> = tables_with_field
                                    .iter()
                                    .map(|t| {
                                        let safe_table = escape_sql_ident(t);
                                        format!(
                                            "SELECT id FROM \"{safe_table}\" \
                                             WHERE \"{safe_col}\" IN (\
                                                SELECT id FROM \"{safe_ref}\" \
                                                WHERE title LIKE '%' || {ph} || '%')"
                                        )
                                    })
                                    .collect();
                                clauses
                                    .push(format!("AND z.id IN ({})", subs.join(" UNION ")));
                                params.push(val.clone());
                            } else if !jt_tables.is_empty() {
                                let ph = format!("?{idx}");
                                idx += 1;
                                let subs: Vec<String> = jt_tables
                                    .iter()
                                    .map(|t| {
                                        let st = escape_sql_ident(t);
                                        let jt = format!("{st}_{safe_col}");
                                        format!(
                                            "SELECT jt.\"{st}_id\" FROM \"{jt}\" jt \
                                             JOIN doogats d ON d.id = jt.\"{safe_col}_id\" \
                                             WHERE d.title LIKE '%' || {ph} || '%'"
                                        )
                                    })
                                    .collect();
                                clauses.push(format!("AND z.id IN ({})", subs.join(" UNION ")));
                                params.push(val.clone());
                            } else {
                                let param_placeholder = format!("?{idx}");
                                idx += 1;
                                let subqueries: Vec<String> = tables_with_field
                                    .iter()
                                    .map(|t| {
                                        let safe_table = escape_sql_ident(t);
                                        format!(
                                            "SELECT id FROM \"{}\" WHERE \"{}\" LIKE '%' || {} || '%'",
                                            safe_table, safe_col, param_placeholder
                                        )
                                    })
                                    .collect();
                                clauses.push(format!("AND z.id IN ({})", subqueries.join(" UNION ")));
                                params.push(val.clone());
                            }
                        }
                        SearchFieldOp::Eq(val) => {
                            let param_placeholder = format!("?{idx}");
                            idx += 1;
                            let subqueries: Vec<String> = tables_with_field
                                .iter()
                                .map(|t| {
                                    let safe_table = escape_sql_ident(t);
                                    format!(
                                        "SELECT id FROM \"{}\" WHERE \"{}\" = {}",
                                        safe_table, safe_col, param_placeholder
                                    )
                                })
                                .collect();
                            let combined = subqueries.join(" UNION ");
                            clauses.push(format!("AND z.id IN ({combined})"));
                            params.push(val.clone());
                        }
                    }
                }
            }
        }

        (clauses, params)
    }
}
