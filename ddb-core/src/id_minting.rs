//! Repo-aware unique-ID existence oracle shared by every mint path (service
//! create / batch / raw-create / bundled-install and the SQL engine's batch
//! INSERT + typedef DDL). PRD 00164.

use std::collections::HashSet;

use rusqlite::Connection;

use crate::error::{DoogatError, Result};
use crate::parser;
use crate::traits::DoogatSource;

/// Build the shared existence oracle. A candidate ID is "taken" when a doogat
/// with that stem exists anywhere under `ddb/` in the repo HEAD tree (root,
/// type folders, or `_typedef/`) OR the `doogats` index holds a row with that
/// id. Both sources are snapshotted once into a single set, so a mint
/// spin/advance loop does not re-query per candidate.
///
/// Fails loud, not free: a repo HEAD-walk error or a real index-query error
/// propagates as `Err`; the one tolerated absence is a missing `doogats` table
/// on a fresh repo (empty index, not an error).
pub(crate) fn existence_oracle<R: DoogatSource + ?Sized>(
    repo: &R,
    conn: &Connection,
) -> Result<impl Fn(&str) -> bool> {
    let mut existing: HashSet<String> = repo
        .list_doogats()?
        .iter()
        .filter_map(|p| parser::extract_id_from_path(p))
        .collect();
    existing.extend(index_ids(conn)?);
    Ok(move |candidate: &str| existing.contains(candidate))
}

/// Snapshot every id in the `doogats` index. A fresh repo whose index schema is
/// not yet created has no `doogats` table — that absence is an empty set, not an
/// error; any other query error propagates.
fn index_ids(conn: &Connection) -> Result<HashSet<String>> {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type = 'table' AND name = 'doogats'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| DoogatError::SqlEngine(format!("id oracle: sqlite_master probe: {e}")))?;
    if !table_exists {
        return Ok(HashSet::new());
    }
    let mut stmt = conn
        .prepare("SELECT id FROM doogats")
        .map_err(|e| DoogatError::SqlEngine(format!("id oracle: prepare: {e}")))?;
    let ids = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| DoogatError::SqlEngine(format!("id oracle: query: {e}")))?
        .collect::<rusqlite::Result<HashSet<String>>>()
        .map_err(|e| DoogatError::SqlEngine(format!("id oracle: row: {e}")))?;
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use crate::git_ops::GitRepo;
    use crate::indexer::Index;
    use crate::traits::{DoogatSource, SqlBackend};
    use crate::types::{CommitHash, DiffKind, DoogatId, DoogatMeta, ParsedDoogat};
    use std::path::Path;
    use tempfile::TempDir;

    /// Commit a doogat file into HEAD without indexing it — makes an ID exist
    /// on-disk (in the HEAD tree) only.
    fn commit_on_disk(repo: &GitRepo, id: &str) {
        repo.commit_file(
            &format!("ddb/{id}.md"),
            &format!("---\ntitle: Doogat {id}\n---\nbody"),
            "add doogat",
        )
        .unwrap();
    }

    /// Build a `ParsedDoogat` for `index.index_doogat`, inserting a `doogats`
    /// index row without writing any git file — makes an ID exist in the index
    /// only.
    fn indexed_doogat(id: &str) -> ParsedDoogat {
        ParsedDoogat {
            meta: DoogatMeta {
                id: Some(DoogatId(id.into())),
                title: Some(format!("Doogat {id}")),
                date: Some("2020-01-01".into()),
                doogat_type: Some("permanent".into()),
                tags: vec![],
                extra: Default::default(),
            },
            body: format!("Body of doogat {id}"),
            sections: vec![],
            reference_section: String::new(),
            inline_fields: vec![],
            links: vec![],
            body_tags: vec![],
            checkboxes: vec![],
            path: format!("ddb/{id}.md"),
            updated_at: None,
        }
    }

    /// A `DoogatSource` whose HEAD walk (`list_doogats`) always fails. Only
    /// `list_doogats` is exercised by `existence_oracle`; the other required
    /// trait methods are inert stubs.
    struct FailingSource;

    impl DoogatSource for FailingSource {
        fn list_doogats(&self) -> crate::error::Result<Vec<String>> {
            Err(crate::error::DoogatError::Git("boom".into()))
        }

        fn read_file(&self, _path: &str) -> crate::error::Result<String> {
            unimplemented!()
        }

        fn head_oid(&self) -> crate::error::Result<CommitHash> {
            unimplemented!()
        }

        fn diff_paths(
            &self,
            _old_oid: &str,
            _new_oid: &str,
        ) -> crate::error::Result<Vec<(DiffKind, String)>> {
            unimplemented!()
        }
    }

    #[test]
    fn reports_on_disk_only_id_as_taken() {
        // ID lives in the HEAD tree but was never indexed → taken via HEAD alone.
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        commit_on_disk(&repo, "20200101000000");

        // Empty index: the `doogats` table exists but holds no row for this id,
        // so an impl that only consults the index would (wrongly) report free.
        let index = Index::open(Path::new(":memory:")).unwrap();

        let oracle = super::existence_oracle(&repo, index.sql_conn()).unwrap();
        assert!(
            oracle("20200101000000"),
            "on-disk id must be taken even when the index has no row for it"
        );
    }

    #[test]
    fn reports_index_only_id_as_taken() {
        // ID lives only in the `doogats` index table, never committed to HEAD.
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        // Decoy commit guarantees HEAD exists; its id differs from the indexed one.
        commit_on_disk(&repo, "20200101000000");

        let index = Index::open(Path::new(":memory:")).unwrap();
        index
            .index_doogat(&indexed_doogat("20200202000000"))
            .unwrap();

        let oracle = super::existence_oracle(&repo, index.sql_conn()).unwrap();
        assert!(
            oracle("20200202000000"),
            "index-only id must be taken even when it is absent from the HEAD tree"
        );
    }

    #[test]
    fn reports_fresh_id_as_free() {
        // Both sources are populated; a candidate absent from both must be free.
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        commit_on_disk(&repo, "20200101000000");

        let index = Index::open(Path::new(":memory:")).unwrap();
        index
            .index_doogat(&indexed_doogat("20200202000000"))
            .unwrap();

        let oracle = super::existence_oracle(&repo, index.sql_conn()).unwrap();
        // Anchor: a populated id is taken, so the oracle is not trivially always-false.
        assert!(oracle("20200101000000"));
        assert!(
            !oracle("20200303000000"),
            "an id absent from both HEAD and index (above the max) must be free"
        );
        // Set-membership, not ordering: an id that sorts BETWEEN the two present
        // ids but is present in neither must be free. Kills a `candidate <= max`
        // range/ordering cheat that a purely above-the-max free assertion misses.
        assert!(
            !oracle("20200150000000"),
            "an id numerically between two present ids, but in neither source, must be free"
        );
    }

    #[test]
    fn empty_when_no_doogats_table() {
        // A bare connection has no `doogats` table; treat it as an empty index
        // (no error), so the oracle reflects only the HEAD tree. Commit an
        // on-disk id so this test ALSO binds that a missing table degrades to an
        // empty index (Ok, not Err) while HEAD is still consulted — an impl that
        // errored on the missing table, or ignored HEAD, fails here.
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        commit_on_disk(&repo, "20200101000000");
        let conn = rusqlite::Connection::open_in_memory().unwrap();

        let oracle = super::existence_oracle(&repo, &conn).unwrap();
        assert!(
            oracle("20200101000000"),
            "missing doogats table must degrade to an empty index (Ok), HEAD still consulted"
        );
        assert!(
            !oracle("20200202000000"),
            "an id absent from HEAD with no index table must be free"
        );
    }

    #[test]
    fn index_only_id_taken_when_head_holds_a_different_id() {
        // Independent second binding of the index-union half so it does not rest
        // on a single test: HEAD holds one id, the index holds another, and the
        // index-only id must be taken. A HEAD-only impl (ignoring the index)
        // fails this, catching the fault Devon's wrong impl exploited.
        let dir = TempDir::new().unwrap();
        let repo = GitRepo::init(dir.path()).unwrap();
        commit_on_disk(&repo, "20200101000000");

        let index = Index::open(Path::new(":memory:")).unwrap();
        index
            .index_doogat(&indexed_doogat("20200404000000"))
            .unwrap();

        let oracle = super::existence_oracle(&repo, index.sql_conn()).unwrap();
        assert!(oracle("20200101000000"), "the HEAD id must be taken");
        assert!(
            oracle("20200404000000"),
            "the index-only id must be taken even though HEAD holds a different id"
        );
    }

    #[test]
    fn propagates_repo_walk_error() {
        // A HEAD-walk failure must surface as Err — never swallowed into "free".
        let index = Index::open(Path::new(":memory:")).unwrap();
        let source = FailingSource;
        assert!(super::existence_oracle(&source, index.sql_conn()).is_err());
    }

    /// Build a forced-collision `exists` closure for `derive_content_id`: the
    /// first two candidates it is asked about are reported taken, everything
    /// after is free. Returns the closure alongside a shared log of the
    /// candidates it reported taken, so a test can assert the function actually
    /// advanced past them rather than ignoring `exists` altogether.
    fn forced_collision_exists() -> (
        std::rc::Rc<std::cell::RefCell<Vec<String>>>,
        impl Fn(&str) -> bool,
    ) {
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let seen_for_closure = std::rc::Rc::clone(&seen);
        let exists = move |candidate: &str| {
            let mut taken = seen_for_closure.borrow_mut();
            if taken.len() < 2 {
                taken.push(candidate.to_string());
                true
            } else {
                false
            }
        };
        (seen, exists)
    }

    #[test]
    fn same_inputs_produce_same_id() {
        // Determinism: same (old_id, blob_oid) pair, no forced collisions, must
        // yield the same id every time — no wall-clock or randomness involved.
        let old_id = "20200101000000";
        let blob_oid = "abc123deadbeef0011223344";

        let first = super::derive_content_id(old_id, blob_oid, |_: &str| false);
        let second = super::derive_content_id(old_id, blob_oid, |_: &str| false);

        assert_eq!(
            first, second,
            "same (old_id, blob_oid) pair must always derive the same id"
        );
    }

    #[test]
    fn different_blob_oid_produces_different_id() {
        // Content-addressed, not just old-ID-addressed: same old_id, different
        // blob_oid must derive a different id. Also kills a constant-return
        // cheat (e.g. always "00000000000000") that same_inputs_produce_same_id
        // alone would not catch.
        let old_id = "20200101000000";

        let from_blob_a =
            super::derive_content_id(old_id, "blob-aaaaaaaaaaaaaaaa", |_: &str| false);
        let from_blob_b =
            super::derive_content_id(old_id, "blob-bbbbbbbbbbbbbbbb", |_: &str| false);

        assert_ne!(
            from_blob_a, from_blob_b,
            "same old_id with a different blob_oid must derive a different id"
        );
    }

    #[test]
    fn advances_past_forced_collisions_deterministically() {
        // Two independent calls, identical (old_id, blob_oid) and identical
        // forced-collision exists closures (first 2 candidates asked report
        // taken, rest free), must converge on the identical final id — and that
        // final id must not be one of the candidates reported taken, proving the
        // function actually consulted `exists` and advanced rather than
        // returning a candidate regardless of collisions.
        let old_id = "20200101000000";
        let blob_oid = "abc123deadbeef0011223344";

        let (seen_first, exists_first) = forced_collision_exists();
        let first = super::derive_content_id(old_id, blob_oid, exists_first);

        let (seen_second, exists_second) = forced_collision_exists();
        let second = super::derive_content_id(old_id, blob_oid, exists_second);

        assert_eq!(
            first, second,
            "two independent calls with identical inputs and identical forced-collision \
             exists closures must converge on the identical final id"
        );

        let taken_candidates = seen_first.borrow();
        assert!(
            !taken_candidates
                .iter()
                .any(|candidate| *candidate == first.0),
            "the final id must not be one of the candidates the exists closure reported taken"
        );
        assert_eq!(
            taken_candidates.len(),
            2,
            "the forced-collision closure must have been consulted for the first 2 candidates"
        );
        drop(taken_candidates);
        assert!(
            !seen_second.borrow().is_empty(),
            "the second independent call must also have consulted its exists closure"
        );
    }

    #[test]
    fn returns_id_with_valid_shape() {
        // Shape: every returned id is exactly 14 ASCII digits, per
        // DoogatId::is_valid_shape.
        let id =
            super::derive_content_id("20200101000000", "abc123deadbeef0011223344", |_: &str| {
                false
            });

        assert!(
            DoogatId::is_valid_shape(&id.0),
            "derived id must satisfy DoogatId::is_valid_shape, got {:?}",
            id.0
        );
    }
}
