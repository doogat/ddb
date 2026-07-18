//! Repo-aware doogat-ID minting support.
//!
//! Currently this file carries only the unit tests for `existence_oracle`
//! (written test-first). The production `existence_oracle` / `index_ids`
//! functions are added by the implementor; until then this module does not
//! compile — that is the expected red state.

#[cfg(test)]
mod tests {
    use crate::git_ops::GitRepo;
    use crate::indexer::Index;
    use crate::traits::{DoogatIndex, DoogatSource, DoogatStore, SqlBackend};
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
}
