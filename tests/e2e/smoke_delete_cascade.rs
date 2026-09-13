use crate::common::{assert_doogat_id, stdout, DdbTestRepo};

#[test]
fn smoke_delete_cascade_matches_sql_and_service_paths() {
    for sql_path in [false, true] {
        let repo = DdbTestRepo::init();
        stdout(&repo, &["query", "CREATE TABLE parent (name TEXT)"]);
        stdout(&repo, &["query", "CREATE TABLE child (name TEXT, first VARCHAR(255) NOT NULL REFERENCES parent(id) ON DELETE CASCADE, second VARCHAR(255) REFERENCES parent(id) ON DELETE CASCADE)"]);
        let parent = stdout(
            &repo,
            &["query", "INSERT INTO parent (name) VALUES ('parent')"],
        );
        assert_doogat_id(&parent);
        let child = stdout(&repo, &["query", &format!("INSERT INTO child (name, first, second) VALUES ('child', '{parent}', '{parent}')")]);
        assert_doogat_id(&child);
        if sql_path {
            assert_eq!(
                stdout(
                    &repo,
                    &[
                        "query",
                        &format!("DELETE FROM parent WHERE id = '{parent}'")
                    ]
                ),
                "1 row(s) affected"
            );
        } else {
            repo.ddb().args(["delete", &parent]).assert().success();
        }
        repo.ddb().args(["read", &child]).assert().failure();
        assert_eq!(
            stdout(&repo, &["query", "SELECT COUNT(*) FROM parent"]),
            "0"
        );
        assert_eq!(stdout(&repo, &["query", "SELECT COUNT(*) FROM child"]), "0");
        assert_eq!(
            stdout(&repo, &["query", "SELECT COUNT(*) FROM child_first"]),
            "0"
        );
        assert_eq!(
            stdout(&repo, &["query", "SELECT COUNT(*) FROM child_second"]),
            "0"
        );
    }
}
